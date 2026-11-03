use super::manager::{ChatConfig, ChatHandle, ChatManager};
use super::types::{
    ApprovalDecision, BranchState, ChatCommand, ChatEvent, SendRejection, TurnOutcome,
};
use crate::ollama::OllamaClient;
use crate::session::{
    AssistantStatus, MessageContent, MessageId, Session, SessionDocument, SessionId,
    SessionLifetime,
};
use crate::store::{Store, Writer};
use crate::test_server::{Route, Step, TcpScript};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::oneshot;

type Events = Arc<Mutex<Vec<ChatEvent>>>;

struct H {
    handle: ChatHandle,
    events: Events,
    dir: TempDir,
}

const SHOW: &str = r#"{"details":{"family":"llama"},"model_info":{"llama.context_length":4096},"capabilities":["completion"],"digest":"sha256:abc"}"#;

fn show_route() -> Route {
    Route::new(
        "/api/show",
        "200 OK",
        "Content-Type: application/json\r\n",
        vec![Step::Write(SHOW.as_bytes().to_vec()), Step::Close],
    )
}

fn chat_route(steps: Vec<Step>) -> Route {
    Route::new("/api/chat", "200 OK", "", steps)
}

fn line(content: &str) -> Vec<u8> {
    format!(
        "{}\n",
        serde_json::json!({"message": {"role": "assistant", "content": content}, "done": false})
    )
    .into_bytes()
}

fn done() -> Vec<u8> {
    b"{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":9}\n".to_vec()
}

/// A complete reply: `text`, then done, then a clean close.
fn reply(text: &str) -> Vec<Step> {
    vec![Step::Write(line(text)), Step::Write(done()), Step::Close]
}

async fn fake(routes: Vec<Route>) -> TcpScript {
    let mut all = vec![show_route()];
    all.extend(routes);
    TcpScript::start_routes(all).await
}

fn client(srv: &TcpScript) -> OllamaClient {
    OllamaClient::new(&srv.base_url())
}

async fn start(srv: &TcpScript) -> H {
    start_in(TempDir::new().unwrap(), client(srv), |_| {}).await
}

async fn start_in(dir: TempDir, c: OllamaClient, tweak: impl FnOnce(&mut ChatConfig)) -> H {
    let deadline = Instant::now() + Duration::from_secs(5);
    let (store, warnings) = loop {
        match Store::open(dir.path()) {
            Ok(v) => break v,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await
            }
            Err(e) => panic!("store never opened: {e}"),
        }
    };
    let writer = Writer::spawn(store, warnings);
    let events: Events = Arc::default();
    let sink_events = Arc::clone(&events);
    let mut cfg = ChatConfig::new(c);
    cfg.coalesce = Duration::from_millis(5);
    tweak(&mut cfg);
    let handle = ChatManager::spawn(
        &tokio::runtime::Handle::current(),
        writer,
        cfg,
        Arc::new(move |e| sink_events.lock().unwrap().push(e)),
    );
    H {
        handle,
        events,
        dir,
    }
}

impl H {
    fn cmd(&self, c: ChatCommand) {
        self.handle.send(c);
    }

    fn events(&self) -> Vec<ChatEvent> {
        self.events.lock().unwrap().clone()
    }

    async fn wait(&self, what: &str, pred: impl Fn(&[ChatEvent]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = self.events();
            if pred(&snapshot) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; events: {:#?}",
                self.events()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn create(&self, lifetime: SessionLifetime) -> SessionId {
        let before = self.opened().len();
        self.cmd(ChatCommand::Create {
            lifetime,
            model: "llama3:8b".into(),
        });
        self.wait("Opened", |e| opened(e).len() > before).await;
        *self.opened().last().unwrap()
    }

    fn opened(&self) -> Vec<SessionId> {
        opened(&self.events())
    }

    async fn send(&self, id: SessionId, text: &str) {
        self.cmd(ChatCommand::Send {
            session: id,
            text: text.into(),
        });
    }

    async fn wait_ended(&self, n: usize) {
        self.wait("TurnEnded", |e| ended(e).len() >= n).await;
    }

    fn deltas(&self, id: SessionId) -> String {
        self.events()
            .iter()
            .filter_map(|e| match e {
                ChatEvent::Delta {
                    session, content, ..
                } if *session == id => Some(content.clone()),
                _ => None,
            })
            .collect()
    }

    fn read(&self, id: SessionId) -> Session {
        read_session(self.dir.path(), id)
    }

    /// `read`, but `None` while the file is not there yet.
    fn try_read(&self, id: SessionId) -> Option<Session> {
        try_read_session(self.dir.path(), id)
    }

    fn file(&self, id: SessionId) -> std::path::PathBuf {
        self.dir.path().join(format!("{id}.json"))
    }
}

fn opened(e: &[ChatEvent]) -> Vec<SessionId> {
    e.iter()
        .filter_map(|e| match e {
            ChatEvent::Opened(s) => Some(s.id),
            _ => None,
        })
        .collect()
}

fn ended(e: &[ChatEvent]) -> Vec<(TurnOutcome, Option<String>)> {
    e.iter()
        .filter_map(|e| match e {
            ChatEvent::TurnEnded {
                outcome, message, ..
            } => Some((*outcome, message.clone())),
            _ => None,
        })
        .collect()
}

fn last_rows(e: &[ChatEvent]) -> Vec<crate::store::IndexRow> {
    e.iter()
        .rev()
        .find_map(|e| match e {
            ChatEvent::Rows(r) => Some(r.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// The saved chat. A file being rewritten at this moment is read again
/// shortly, so a test never trips over a half-written one.
fn read_session(dir: &Path, id: SessionId) -> Session {
    for _ in 0..200 {
        if let Some(s) = try_read_session(dir, id) {
            return s;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the saved chat {id} could not be read");
}

/// The saved chat, or `None` while its file is not (completely) there yet.
fn try_read_session(dir: &Path, id: SessionId) -> Option<Session> {
    let bytes = std::fs::read(dir.join(format!("{id}.json"))).ok()?;
    serde_json::from_slice::<SessionDocument>(&bytes)
        .ok()
        .map(|d| d.session)
}

fn preload(dir: &Path, s: &Session) {
    let (mut st, _) = Store::open(dir).unwrap();
    st.save(s).unwrap();
}

fn texts(s: &Session) -> Vec<(String, String)> {
    s.messages
        .iter()
        .map(|m| match &m.content {
            MessageContent::User { text, .. } => ("user".into(), text.clone()),
            MessageContent::Assistant { text, status, .. } => {
                (format!("assistant:{status:?}"), text.clone())
            }
            MessageContent::Tool { .. } => ("tool".into(), String::new()),
        })
        .collect()
}

fn chat_bodies(srv: &TcpScript) -> Vec<serde_json::Value> {
    srv.requests()
        .into_iter()
        .filter(|(p, _)| p == "/api/chat")
        .map(|(_, b)| serde_json::from_str(&b).unwrap())
        .collect()
}

fn dir_snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name() != ".lock")
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap_or_default(),
            )
        })
        .collect();
    v.sort();
    v
}

// ---------------------------------------------------------------- lifecycle

#[tokio::test]
async fn create_persisted_chat_appears_in_rows_and_on_disk() {
    let srv = fake(vec![]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.wait("rows", |e| last_rows(e).iter().any(|r| r.id == id))
        .await;
    assert!(h.file(id).exists());
}

#[tokio::test]
async fn create_incognito_appears_in_rows_but_writes_nothing_to_disk() {
    let srv = fake(vec![]).await;
    let h = start(&srv).await;
    let before = dir_snapshot(h.dir.path());
    let id = h.create(SessionLifetime::Incognito).await;
    h.wait("rows", |e| last_rows(e).iter().any(|r| r.id == id))
        .await;
    let row = last_rows(&h.events())
        .into_iter()
        .find(|r| r.id == id)
        .unwrap();
    assert_eq!(row.lifetime, SessionLifetime::Incognito);
    assert_eq!(dir_snapshot(h.dir.path()), before);
}

#[tokio::test]
async fn open_returns_the_transcript() {
    let srv = fake(vec![chat_route(reply("Hello"))]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "hi").await;
    h.wait_ended(1).await;
    h.cmd(ChatCommand::Close(id));
    h.cmd(ChatCommand::Open(id));
    h.wait("reopen", |e| opened(e).len() == 2).await;
    let reopened = h
        .events()
        .into_iter()
        .rev()
        .find_map(|e| match e {
            ChatEvent::Opened(s) => Some(s),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        texts(&reopened),
        vec![
            ("user".into(), "hi".into()),
            ("assistant:Complete".into(), "Hello".into())
        ]
    );
}

#[tokio::test]
async fn rename_and_pin_do_not_change_updated_at_or_order() {
    let srv = fake(vec![]).await;
    let h = start(&srv).await;
    let a = h.create(SessionLifetime::Persisted).await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let b = h.create(SessionLifetime::Persisted).await;
    h.wait("rows", |e| last_rows(e).len() == 2).await;
    let before = last_rows(&h.events());
    h.cmd(ChatCommand::Rename(a, "Renamed".into()));
    h.cmd(ChatCommand::SetPinned(a, true));
    h.wait("rename+pin", |e| {
        last_rows(e)
            .iter()
            .any(|r| r.id == a && r.pinned && r.title == "Renamed")
    })
    .await;
    let after = last_rows(&h.events());
    let upd = |rows: &[crate::store::IndexRow], id| {
        rows.iter().find(|r| r.id == id).unwrap().updated_at_ms
    };
    assert_eq!(upd(&before, a), upd(&after, a));
    assert_eq!(
        after.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![b, a],
        "order must not change"
    );
}

#[tokio::test]
async fn closing_incognito_destroys_it() {
    let srv = fake(vec![]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Incognito).await;
    h.cmd(ChatCommand::Close(id));
    h.wait("gone", |e| !last_rows(e).iter().any(|r| r.id == id))
        .await;
    h.cmd(ChatCommand::Open(id));
    h.wait("error", |e| {
        e.iter()
            .any(|e| matches!(e, ChatEvent::SessionError { session: Some(s), .. } if *s == id))
    })
    .await;
}

#[tokio::test]
async fn clear_incognito_destroys_all_and_keeps_saved_chats() {
    let srv = fake(vec![]).await;
    let h = start(&srv).await;
    let saved = h.create(SessionLifetime::Persisted).await;
    let i1 = h.create(SessionLifetime::Incognito).await;
    let i2 = h.create(SessionLifetime::Incognito).await;
    h.cmd(ChatCommand::ClearIncognito);
    h.wait("cleared", |e| {
        let ids: Vec<_> = last_rows(e).iter().map(|r| r.id).collect();
        ids.contains(&saved) && !ids.contains(&i1) && !ids.contains(&i2)
    })
    .await;
}

#[tokio::test]
async fn delete_removes_file_and_row() {
    let srv = fake(vec![]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.cmd(ChatCommand::Delete(id));
    h.wait("deleted", |e| {
        !last_rows(e).iter().any(|r| r.id == id) && e.len() > 3
    })
    .await;
    assert!(!h.file(id).exists());
}

#[tokio::test]
async fn set_model_changes_model_when_idle() {
    let srv = fake(vec![]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.cmd(ChatCommand::SetModel(id, "qwen:7b".into()));
    h.wait("model", |e| {
        last_rows(e)
            .iter()
            .any(|r| r.id == id && r.model == "qwen:7b")
    })
    .await;
}

#[tokio::test]
async fn restart_lists_saved_sessions_again() {
    let srv = fake(vec![]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.wait("rows", |e| last_rows(e).iter().any(|r| r.id == id))
        .await;
    h.handle.shutdown().await;
    let H { dir, .. } = h;
    let h2 = start_in(dir, client(&srv), |_| {}).await;
    h2.wait("rows", |e| last_rows(e).iter().any(|r| r.id == id))
        .await;
}

// ---------------------------------------------------------------- runs

#[tokio::test]
async fn a_chat_streams_and_completes_and_is_saved() {
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("Hel")),
        Step::Write(line("lo")),
        Step::Write(done()),
        Step::Close,
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "hi").await;
    h.wait_ended(1).await;
    assert_eq!(ended(&h.events())[0].0, TurnOutcome::Completed);
    assert_eq!(h.deltas(id), "Hello");
    h.wait("saved", |_| {
        texts(&h.read(id)).last().is_some_and(|m| m.1 == "Hello")
    })
    .await;
    assert_eq!(
        texts(&h.read(id)),
        vec![
            ("user".into(), "hi".into()),
            ("assistant:Complete".into(), "Hello".into())
        ]
    );
}

#[tokio::test]
async fn the_request_carries_num_ctx_equal_to_the_trim_window() {
    let srv = fake(vec![chat_route(reply("ok"))]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "hi").await;
    h.wait_ended(1).await;
    let bodies = chat_bodies(&srv);
    assert_eq!(bodies[0]["options"]["num_ctx"], 4096);
    assert_eq!(bodies[0]["messages"][0]["role"], "system");
    assert_eq!(bodies[0]["messages"][1]["content"], "hi");
}

#[tokio::test]
async fn a_second_send_while_running_is_rejected_busy() {
    let (release, hold) = oneshot::channel();
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("a")),
        Step::WaitFor(hold),
        Step::Write(done()),
        Step::Close,
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "one").await;
    h.wait("streaming", |_| !h.deltas(id).is_empty()).await;
    h.send(id, "two").await;
    h.wait("rejected", |e| {
        e.iter().any(|e| {
            matches!(
                e,
                ChatEvent::SendRejected {
                    reason: SendRejection::Busy,
                    ..
                }
            )
        })
    })
    .await;
    release.send(()).unwrap();
    h.wait_ended(1).await;
    assert_eq!(
        texts(&h.read(id)).iter().filter(|m| m.0 == "user").count(),
        1
    );
}

#[tokio::test]
async fn stop_cancels_and_keeps_the_partial_text_labelled_cancelled() {
    let (_keep, hold) = oneshot::channel::<()>();
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("par")),
        Step::WaitFor(hold),
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait("partial", |_| h.deltas(id) == "par").await;
    h.cmd(ChatCommand::Stop(id));
    h.wait_ended(1).await;
    assert_eq!(ended(&h.events())[0].0, TurnOutcome::Cancelled);
    h.wait("saved", |_| {
        texts(&h.read(id))
            .last()
            .is_some_and(|m| m.0 == "assistant:Cancelled")
    })
    .await;
    assert_eq!(texts(&h.read(id)).last().unwrap().1, "par");
}

#[tokio::test]
async fn stop_racing_a_held_done_line_yields_one_terminal_cancelled() {
    let (release, hold) = oneshot::channel();
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("a")),
        Step::WaitFor(hold),
        Step::Write(done()),
        Step::Close,
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait("partial", |_| h.deltas(id) == "a").await;
    h.cmd(ChatCommand::Stop(id));
    h.wait_ended(1).await;
    let seen = h.events().len();
    let _ = release.send(());
    tokio::time::sleep(Duration::from_millis(150)).await;
    let all = h.events();
    assert_eq!(ended(&all).len(), 1);
    assert_eq!(ended(&all)[0].0, TurnOutcome::Cancelled);
    assert!(
        !all[seen..]
            .iter()
            .any(|e| matches!(e, ChatEvent::Delta { .. } | ChatEvent::TurnEnded { .. })),
        "events after the terminal one: {:?}",
        &all[seen..]
    );
}

#[tokio::test]
async fn completion_then_stop_yields_one_terminal_completed() {
    let srv = fake(vec![chat_route(reply("done"))]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait_ended(1).await;
    h.cmd(ChatCommand::Stop(id));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(ended(&h.events()), vec![(TurnOutcome::Completed, None)]);
}

#[tokio::test]
async fn early_close_without_done_fails_and_keeps_partial_text() {
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("half")),
        Step::Close,
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait_ended(1).await;
    let (outcome, msg) = ended(&h.events())[0].clone();
    assert_eq!(outcome, TurnOutcome::Failed);
    assert!(msg.unwrap().contains("connection ended early"));
    h.wait("saved", |_| {
        texts(&h.read(id))
            .last()
            .is_some_and(|m| m.0 == "assistant:Failed")
    })
    .await;
    assert_eq!(texts(&h.read(id)).last().unwrap().1, "half");
}

#[tokio::test]
async fn a_stall_fails_once() {
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("x")),
        Step::Pause(Duration::from_secs(5)),
    ])])
    .await;
    let c = client(&srv).with_stall_timeout(Duration::from_millis(150));
    let h = start_in(TempDir::new().unwrap(), c, |_| {}).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait_ended(1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let all = ended(&h.events());
    assert_eq!(all.len(), 1);
    assert!(all[0].1.as_ref().unwrap().contains("stopped responding"));
}

#[tokio::test]
async fn unreachable_ollama_fails_once_with_a_readable_message() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let c = OllamaClient::new(&format!("http://127.0.0.1:{port}"));
    let h = start_in(TempDir::new().unwrap(), c, |_| {}).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait_ended(1).await;
    let all = ended(&h.events());
    assert_eq!(all.len(), 1);
    assert!(all[0].1.as_ref().unwrap().contains("not reachable"));
}

#[tokio::test]
async fn failed_before_the_first_token_leaves_an_empty_error_message() {
    let srv = fake(vec![Route::new(
        "/api/chat",
        "500 Internal Server Error",
        "",
        vec![
            Step::Write(b"{\"error\":\"out of memory\"}".to_vec()),
            Step::Close,
        ],
    )])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait_ended(1).await;
    h.wait("saved", |_| {
        texts(&h.read(id))
            .get(1)
            .is_some_and(|m| m.0 == "assistant:Failed")
    })
    .await;
    let s = h.read(id);
    assert!(matches!(
        &s.messages[1].content,
        MessageContent::Assistant { text, status: AssistantStatus::Failed, error: Some(e), .. }
            if text.is_empty() && e.contains("out of memory")
    ));
}

#[tokio::test]
async fn a_404_emits_model_missing() {
    let srv = fake(vec![Route::new(
        "/api/chat",
        "404 Not Found",
        "",
        vec![
            Step::Write(b"{\"error\":\"model not found\"}".to_vec()),
            Step::Close,
        ],
    )])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait_ended(1).await;
    assert!(h
        .events()
        .iter()
        .any(|e| matches!(e, ChatEvent::ModelMissing { model, .. } if model == "llama3:8b")));
}

#[tokio::test]
async fn retry_after_failure_appends_a_new_reply_and_excludes_the_failed_one_from_context() {
    let srv = fake(vec![
        chat_route(vec![Step::Write(line("half")), Step::Close]),
        chat_route(reply("ok")),
    ])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "ask").await;
    h.wait_ended(1).await;
    h.cmd(ChatCommand::Retry(id));
    h.wait_ended(2).await;
    assert_eq!(ended(&h.events())[1].0, TurnOutcome::Completed);
    h.wait("saved", |_| {
        texts(&h.read(id)).len() == 3 && texts(&h.read(id))[2].1 == "ok"
    })
    .await;
    assert_eq!(
        texts(&h.read(id))[1],
        ("assistant:Failed".into(), "half".into())
    );
    let second = &chat_bodies(&srv)[1]["messages"];
    assert_eq!(second.as_array().unwrap().len(), 2, "{second}");
    assert_eq!(second[1]["content"], "ask");
}

#[tokio::test]
async fn a_crash_interrupted_last_turn_offers_retry_and_resends() {
    let dir = TempDir::new().unwrap();
    let mut s = Session::new_chat("llama3:8b", SessionLifetime::Persisted, 10);
    s.push_user("question", 11);
    let a = s.begin_assistant(
        crate::session::RunId::new(),
        crate::session::TurnId::new(),
        12,
    );
    s.append_assistant(a, "part", "");
    preload(dir.path(), &s);
    let srv = fake(vec![chat_route(reply("answer"))]).await;
    let h = start_in(dir, client(&srv), |_| {}).await;
    h.cmd(ChatCommand::Retry(s.id));
    h.wait_ended(1).await;
    assert_eq!(ended(&h.events())[0].0, TurnOutcome::Completed);
    assert_eq!(chat_bodies(&srv)[0]["messages"][1]["content"], "question");
}

#[tokio::test]
async fn a_new_message_after_a_failed_exchange_sends_no_consecutive_user_turns() {
    let srv = fake(vec![
        chat_route(vec![Step::Write(line("half")), Step::Close]),
        chat_route(reply("fine")),
    ])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "first").await;
    h.wait_ended(1).await;
    h.send(id, "second").await;
    h.wait_ended(2).await;
    let roles: Vec<String> = chat_bodies(&srv)[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(roles, ["system", "user"]);
    assert_eq!(chat_bodies(&srv)[1]["messages"][1]["content"], "second");
}

#[tokio::test]
async fn long_history_is_trimmed_with_the_marker_and_the_transcript_is_untouched() {
    let dir = TempDir::new().unwrap();
    let mut s = Session::new_chat("llama3:8b", SessionLifetime::Persisted, 10);
    for i in 0..40 {
        s.push_user(&format!("q{i} {}", "x".repeat(600)), 11);
        let a = s.begin_assistant(
            crate::session::RunId::new(),
            crate::session::TurnId::new(),
            12,
        );
        s.append_assistant(a, &"y".repeat(600), "");
        s.finish_assistant(a, AssistantStatus::Complete, None, 13);
    }
    preload(dir.path(), &s);
    let srv = fake(vec![chat_route(reply("ok"))]).await;
    let h = start_in(dir, client(&srv), |_| {}).await;
    h.send(s.id, "latest").await;
    h.wait_ended(1).await;
    assert!(h
        .events()
        .iter()
        .any(|e| matches!(e, ChatEvent::ContextTrimmed { omitted, .. } if *omitted > 0)));
    let sent = chat_bodies(&srv)[0]["messages"].as_array().unwrap().clone();
    assert_eq!(sent[1]["content"], "Earlier messages omitted for context");
    assert!(sent.len() < 80);
    h.wait("saved", |_| h.read(s.id).messages.len() == 82).await;
}

#[tokio::test]
async fn an_oversized_message_is_rejected_too_large_and_nothing_is_sent_or_saved() {
    let srv = fake(vec![chat_route(reply("never"))]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, &"x".repeat(30_000)).await;
    h.wait("rejected", |e| {
        e.iter().any(|e| {
            matches!(
                e,
                ChatEvent::SendRejected {
                    reason: SendRejection::TooLarge(_),
                    ..
                }
            )
        })
    })
    .await;
    assert!(chat_bodies(&srv).is_empty());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(h.read(id).messages.is_empty());
}

#[tokio::test]
async fn deltas_are_coalesced_and_arrive_before_the_terminal_event() {
    let mut steps: Vec<Step> = (0..20)
        .map(|i| Step::Write(line(&format!("{i},"))))
        .collect();
    steps.push(Step::Write(done()));
    steps.push(Step::Close);
    let srv = fake(vec![chat_route(steps)]).await;
    let c = client(&srv);
    let h = start_in(TempDir::new().unwrap(), c, |cfg| {
        cfg.coalesce = Duration::from_millis(80)
    })
    .await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait_ended(1).await;
    let events = h.events();
    let delta_count = events
        .iter()
        .filter(|e| matches!(e, ChatEvent::Delta { .. }))
        .count();
    assert!(delta_count < 20, "{delta_count} deltas");
    let last_delta = events
        .iter()
        .rposition(|e| matches!(e, ChatEvent::Delta { .. }))
        .unwrap();
    let end = events
        .iter()
        .position(|e| matches!(e, ChatEvent::TurnEnded { .. }))
        .unwrap();
    assert!(last_delta < end);
    let expected: String = (0..20).map(|i| format!("{i},")).collect();
    assert_eq!(h.deltas(id), expected);
}

#[tokio::test]
async fn partial_text_is_flushed_to_disk_while_streaming() {
    let (release, hold) = oneshot::channel();
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("par")),
        Step::WaitFor(hold),
        Step::Write(done()),
        Step::Close,
    ])])
    .await;
    let h = start_in(TempDir::new().unwrap(), client(&srv), |cfg| {
        cfg.flush_interval = Duration::from_millis(40)
    })
    .await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait("flushed", |_| {
        std::fs::read(h.file(id))
            .ok()
            .and_then(|b| serde_json::from_slice::<SessionDocument>(&b).ok())
            .is_some_and(|d| texts(&d.session).last().is_some_and(|m| m.1 == "par"))
    })
    .await;
    release.send(()).unwrap();
    h.wait_ended(1).await;
}

#[tokio::test]
async fn two_sessions_run_concurrently() {
    let (r1, h1) = oneshot::channel();
    let (r2, h2) = oneshot::channel();
    let srv = fake(vec![
        chat_route(vec![
            Step::Write(line("a")),
            Step::WaitFor(h1),
            Step::Write(done()),
            Step::Close,
        ]),
        chat_route(vec![
            Step::Write(line("b")),
            Step::WaitFor(h2),
            Step::Write(done()),
            Step::Close,
        ]),
    ])
    .await;
    let h = start(&srv).await;
    let s1 = h.create(SessionLifetime::Persisted).await;
    let s2 = h.create(SessionLifetime::Persisted).await;
    h.send(s1, "one").await;
    h.wait("first streaming", |_| !h.deltas(s1).is_empty())
        .await;
    h.send(s2, "two").await;
    h.wait("second streaming", |_| !h.deltas(s2).is_empty())
        .await;
    assert!(
        ended(&h.events()).is_empty(),
        "both must be running at once"
    );
    assert!(h
        .events()
        .iter()
        .any(|e| matches!(e, ChatEvent::WaitingForModel { session, .. } if *session == s2)));
    r1.send(()).unwrap();
    r2.send(()).unwrap();
    h.wait_ended(2).await;
}

// ---------------------------------------------------------------- teardown

#[tokio::test]
async fn delete_during_a_run_leaves_no_file_even_with_a_late_flush() {
    let (_keep, hold) = oneshot::channel::<()>();
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("x")),
        Step::WaitFor(hold),
    ])])
    .await;
    let h = start_in(TempDir::new().unwrap(), client(&srv), |cfg| {
        cfg.flush_interval = Duration::from_millis(15)
    })
    .await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait("streaming", |_| !h.deltas(id).is_empty()).await;
    h.cmd(ChatCommand::Delete(id));
    h.wait("rows without it", |e| {
        e.iter()
            .rev()
            .take(3)
            .any(|e| matches!(e, ChatEvent::Rows(r) if r.iter().all(|x| x.id != id)))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!h.file(id).exists());
    assert!(
        ended(&h.events()).is_empty(),
        "a deleted session emits no terminal event"
    );
}

#[tokio::test]
async fn closing_a_running_saved_session_keeps_the_final_flush() {
    let (release, hold) = oneshot::channel();
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("one ")),
        Step::WaitFor(hold),
        Step::Write(line("two")),
        Step::Write(done()),
        Step::Close,
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait("streaming", |_| !h.deltas(id).is_empty()).await;
    h.cmd(ChatCommand::Close(id));
    release.send(()).unwrap();
    h.wait_ended(1).await;
    h.wait("saved", |_| {
        texts(&h.read(id)).last().is_some_and(|m| m.1 == "one two")
    })
    .await;
}

#[tokio::test]
async fn closing_a_running_incognito_session_cancels_and_destroys_it() {
    let (_keep, hold) = oneshot::channel::<()>();
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("x")),
        Step::WaitFor(hold),
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Incognito).await;
    h.send(id, "go").await;
    h.wait("streaming", |_| !h.deltas(id).is_empty()).await;
    h.cmd(ChatCommand::Close(id));
    h.wait("gone", |e| !last_rows(e).iter().any(|r| r.id == id))
        .await;
}

#[tokio::test]
async fn shutdown_flushes_a_cancelled_partial_within_the_limit() {
    let (_keep, hold) = oneshot::channel::<()>();
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("partial")),
        Step::WaitFor(hold),
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait("streaming", |_| h.deltas(id) == "partial").await;
    let t = Instant::now();
    h.handle.shutdown().await;
    assert!(t.elapsed() < Duration::from_secs(3));
    let s = h.read(id);
    assert_eq!(
        texts(&s).last().unwrap(),
        &("assistant:Cancelled".to_string(), "partial".to_string())
    );
    assert!(s.is_idle());
}

#[tokio::test]
async fn a_full_incognito_conversation_leaves_the_data_dir_byte_identical() {
    let srv = fake(vec![chat_route(reply("secret reply"))]).await;
    let h = start(&srv).await;
    let before = dir_snapshot(h.dir.path());
    let id = h.create(SessionLifetime::Incognito).await;
    h.send(id, "secret question").await;
    h.wait_ended(1).await;
    h.cmd(ChatCommand::Rename(id, "secret title".into()));
    h.cmd(ChatCommand::SetPinned(id, true));
    h.cmd(ChatCommand::Close(id));
    h.cmd(ChatCommand::ClearIncognito);
    h.handle.shutdown().await;
    let after = dir_snapshot(h.dir.path());
    assert_eq!(after, before);
    for (name, bytes) in &after {
        assert!(!String::from_utf8_lossy(bytes).contains("secret"), "{name}");
    }
}

#[tokio::test]
async fn opening_a_running_session_flushes_pending_text_before_the_snapshot() {
    let (_keep, hold) = oneshot::channel::<()>();
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("a")),
        Step::Write(line("b")),
        Step::WaitFor(hold),
    ])])
    .await;
    // A long batching window keeps the text pending when Open arrives.
    let h = start_in(TempDir::new().unwrap(), client(&srv), |cfg| {
        cfg.coalesce = Duration::from_secs(30)
    })
    .await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait("both chunks arrived", |_| {
        srv.requests().iter().any(|(p, _)| p == "/api/chat")
    })
    .await;
    // At every Open, the text flushed before the snapshot must equal the
    // snapshot's text. Keep opening until both chunks have arrived.
    let mut reached = false;
    for round in 1..=100 {
        h.cmd(ChatCommand::Open(id));
        h.wait("reopened", |e| opened(e).len() == round + 1).await;
        let events = h.events();
        let at = events
            .iter()
            .rposition(|e| matches!(e, ChatEvent::Opened(_)))
            .unwrap();
        let ChatEvent::Opened(snapshot) = &events[at] else {
            unreachable!()
        };
        let snap_text = texts(snapshot).last().unwrap().1.clone();
        let flushed_before: String = events[..at]
            .iter()
            .filter_map(|e| match e {
                ChatEvent::Delta { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            flushed_before, snap_text,
            "pending text must be sent before the snapshot"
        );
        if snap_text == "ab" {
            reached = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(reached, "both chunks should have arrived");
}

#[tokio::test]
async fn a_reply_cut_off_by_the_length_limit_is_failed_not_completed() {
    let cut = b"{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"done_reason\":\"length\"}\n".to_vec();
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("half an ans")),
        Step::Write(cut),
        Step::Close,
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait_ended(1).await;
    let (outcome, msg) = ended(&h.events())[0].clone();
    assert_eq!(outcome, TurnOutcome::Failed);
    assert!(msg.unwrap().contains("length limit"));
    h.wait("saved", |_| {
        texts(&h.read(id))
            .last()
            .is_some_and(|m| m.0 == "assistant:Failed")
    })
    .await;
    assert_eq!(texts(&h.read(id)).last().unwrap().1, "half an ans");
}

#[tokio::test]
async fn an_unchanged_transcript_is_not_rewritten_on_every_flush() {
    let (release, hold) = oneshot::channel();
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("x")),
        Step::WaitFor(hold),
        Step::Write(done()),
        Step::Close,
    ])])
    .await;
    let h = start_in(TempDir::new().unwrap(), client(&srv), |cfg| {
        cfg.flush_interval = Duration::from_millis(20)
    })
    .await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait("flushed", |_| {
        std::fs::read(h.file(id)).is_ok_and(|b| String::from_utf8_lossy(&b).contains("\"x\""))
    })
    .await;
    // Nothing new is streaming now: the file must stop being rewritten.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stamp = std::fs::metadata(h.file(id)).unwrap().modified().unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        std::fs::metadata(h.file(id)).unwrap().modified().unwrap(),
        stamp
    );
    release.send(()).unwrap();
    h.wait_ended(1).await;
}

// ------------------------------------------------------------------ agents

const SHOW_TOOLS: &str = r#"{"details":{"family":"llama"},"model_info":{"llama.context_length":8192},"capabilities":["completion","tools"],"digest":"sha256:abc"}"#;

fn show_tools_route() -> Route {
    Route::new(
        "/api/show",
        "200 OK",
        "Content-Type: application/json\r\n",
        vec![Step::Write(SHOW_TOOLS.as_bytes().to_vec()), Step::Close],
    )
}

async fn fake_agent(routes: Vec<Route>) -> TcpScript {
    let mut all = vec![show_tools_route()];
    all.extend(routes);
    TcpScript::start_routes(all).await
}

fn tool_call(name: &str, args: serde_json::Value) -> Vec<u8> {
    format!(
        "{}\n",
        serde_json::json!({
            "message": {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": name, "arguments": args}}]},
            "done": false
        })
    )
    .into_bytes()
}

/// A model step that asks for one tool.
fn calls(name: &str, args: serde_json::Value) -> Vec<Step> {
    vec![
        Step::Write(tool_call(name, args)),
        Step::Write(done()),
        Step::Close,
    ]
}

fn run_ends(e: &[ChatEvent]) -> Vec<(super::types::RunOutcome, Option<String>)> {
    e.iter()
        .filter_map(|e| match e {
            ChatEvent::RunEnded {
                outcome, message, ..
            } => Some((*outcome, message.clone())),
            _ => None,
        })
        .collect()
}

fn workspace() -> TempDir {
    let w = TempDir::new().unwrap();
    std::fs::write(w.path().join("a.txt"), "hello\nworld\n").unwrap();
    std::fs::write(w.path().join(".env"), "TOKEN=1").unwrap();
    w
}

impl H {
    async fn create_agent(&self, ws: &TempDir, lifetime: SessionLifetime) -> SessionId {
        let before = self.opened().len();
        self.cmd(ChatCommand::CreateAgent {
            lifetime,
            model: "llama3:8b".into(),
            workspace: Some(ws.path().to_path_buf()),
        });
        self.wait("agent opened", |e| opened(e).len() > before)
            .await;
        *self.opened().last().unwrap()
    }

    async fn wait_run_ended(&self, n: usize) {
        self.wait("RunEnded", |e| run_ends(e).len() >= n).await;
    }
}

#[tokio::test]
async fn an_agent_reads_a_file_and_answers() {
    let ws = workspace();
    let srv = fake_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("It says hello.")),
    ])
    .await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.send(id, "what is in a.txt?").await;
    h.wait_run_ended(1).await;
    let events = h.events();
    assert_eq!(
        run_ends(&events),
        vec![(super::types::RunOutcome::Completed, None)]
    );
    let started: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ChatEvent::ToolStarted {
                name, arguments, ..
            } => Some((name.clone(), arguments.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        [(
            "read_file".to_string(),
            serde_json::json!({"path": "a.txt"})
        )]
    );
    assert!(events.iter().any(|e| matches!(e, ChatEvent::ToolFinished { ok: true, output, .. } if output == "1| hello\n2| world")));
    assert_eq!(ended(&events).len(), 2, "one turn per model step");

    // What the model saw.
    let bodies = chat_bodies(&srv);
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["tools"].as_array().unwrap().len(), 9);
    let system = bodies[0]["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.contains("an agent inside the attached workspace"),
        "{system}"
    );
    assert!(
        system.contains(&ws.path().display().to_string()),
        "{system}"
    );
    assert!(system.contains("Steps remaining: 20"), "{system}");
    let second = bodies[1]["messages"].as_array().unwrap();
    let roles: Vec<_> = second.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["system", "user", "assistant", "tool"]);
    assert_eq!(second[2]["tool_calls"][0]["function"]["name"], "read_file");
    assert_eq!(second[3]["tool_name"], "read_file");
    assert_eq!(second[3]["content"], "1| hello\n2| world");

    // What was saved.
    h.wait("saved", |_| {
        let t = texts(&h.read(id));
        t.last().is_some_and(|m| m.1 == "It says hello.")
    })
    .await;
    let s = h.read(id);
    assert_eq!(s.mode, crate::session::SessionMode::Agent);
    assert_eq!(s.workspace.as_deref(), Some(ws.path()));
    let kinds: Vec<_> = texts(&s).iter().map(|m| m.0.clone()).collect();
    assert_eq!(
        kinds,
        ["user", "assistant:Complete", "tool", "assistant:Complete"]
    );
    assert!(s.is_idle());
}

#[tokio::test]
async fn a_model_without_tool_support_cannot_run_an_agent() {
    let ws = workspace();
    let srv = fake(vec![chat_route(reply("never"))]).await; // show without "tools"
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.send(id, "hello").await;
    h.wait("rejected", |e| {
        e.iter().any(|e| matches!(e, ChatEvent::SendRejected { reason: SendRejection::NoToolSupport(m), .. } if m == "llama3:8b"))
    })
    .await;
    assert!(chat_bodies(&srv).is_empty());
    assert!(h.read(id).messages.is_empty());
}

#[tokio::test]
async fn check_model_reports_whether_tools_are_supported() {
    let srv = fake_agent(vec![]).await;
    let h = start(&srv).await;
    h.cmd(ChatCommand::CheckModel("llama3:8b".into()));
    h.wait("capabilities", |e| {
        e.iter().any(|e| matches!(e, ChatEvent::ModelCapabilities { model, tools: true } if model == "llama3:8b"))
    })
    .await;
    let plain = fake(vec![]).await;
    let h2 = start(&plain).await;
    h2.cmd(ChatCommand::CheckModel("llama3:8b".into()));
    h2.wait("capabilities", |e| {
        e.iter()
            .any(|e| matches!(e, ChatEvent::ModelCapabilities { tools: false, .. }))
    })
    .await;
}

#[tokio::test]
async fn tool_failures_go_back_to_the_model_and_the_run_continues() {
    let ws = workspace();
    let srv = fake_agent(vec![
        chat_route(calls(
            "read_file",
            serde_json::json!({"path": "../etc/passwd"}),
        )),
        chat_route(calls("read_file", serde_json::json!({"path": ".env"}))),
        chat_route(reply("I could not read those.")),
    ])
    .await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.send(id, "read secrets").await;
    h.wait_run_ended(1).await;
    let events = h.events();
    assert_eq!(run_ends(&events)[0].0, super::types::RunOutcome::Completed);
    let failures: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            ChatEvent::ToolFinished {
                ok: false, output, ..
            } => Some(output.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(failures.len(), 2);
    assert!(
        failures[0].starts_with("Error: ") && failures[0].contains("parent directory"),
        "{failures:?}"
    );
    assert!(failures[1].contains("protected"), "{failures:?}");
    let last = &chat_bodies(&srv)[2]["messages"];
    assert!(last
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool" && m["content"].as_str().unwrap().contains("protected")));
}

#[tokio::test]
async fn the_step_budget_ends_a_run_that_keeps_calling_tools() {
    let ws = workspace();
    let srv = fake_agent(vec![
        chat_route(calls("list_dir", serde_json::json!({}))),
        chat_route(calls("list_dir", serde_json::json!({}))),
        chat_route(reply("unreachable")),
    ])
    .await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.cmd(ChatCommand::SetStepBudget(id, 2));
    h.send(id, "list forever").await;
    h.wait_run_ended(1).await;
    let (outcome, msg) = run_ends(&h.events())[0].clone();
    assert_eq!(outcome, super::types::RunOutcome::BudgetExhausted);
    assert!(msg.unwrap().contains("all 2 steps"));
    assert_eq!(chat_bodies(&srv).len(), 2, "no third request");
    let first_system = chat_bodies(&srv)[0]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(first_system.contains("Steps remaining: 2"));
    assert!(chat_bodies(&srv)[1]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("Steps remaining: 1"));
}

#[tokio::test]
async fn stop_ends_a_streaming_agent_run_once() {
    let ws = workspace();
    let (_keep, hold) = oneshot::channel::<()>();
    let srv = fake_agent(vec![chat_route(vec![
        Step::Write(line("thinking")),
        Step::WaitFor(hold),
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait("streaming", |_| h.deltas(id) == "thinking").await;
    h.cmd(ChatCommand::Stop(id));
    h.wait_run_ended(1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let events = h.events();
    assert_eq!(
        run_ends(&events),
        vec![(super::types::RunOutcome::Cancelled, None)]
    );
    assert_eq!(ended(&events), vec![(TurnOutcome::Cancelled, None)]);
    h.wait("saved", |_| {
        texts(&h.read(id))
            .last()
            .is_some_and(|m| m.0 == "assistant:Cancelled")
    })
    .await;
}

#[tokio::test]
async fn steering_interrupts_the_reply_keeps_it_and_continues_the_same_run() {
    let ws = workspace();
    let (_keep, hold) = oneshot::channel::<()>();
    let srv = fake_agent(vec![
        chat_route(vec![Step::Write(line("partial")), Step::WaitFor(hold)]),
        chat_route(reply("ok, switching")),
    ])
    .await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.send(id, "do the first thing").await;
    h.wait("streaming", |_| h.deltas(id) == "partial").await;
    h.send(id, "do the other thing instead").await;
    h.wait_run_ended(1).await;
    let events = h.events();
    assert_eq!(
        run_ends(&events),
        vec![(super::types::RunOutcome::Completed, None)],
        "one run, one end"
    );
    assert!(events.iter().any(|e| matches!(e, ChatEvent::SteeringQueued { text, .. } if text == "do the other thing instead")));
    assert_eq!(
        ended(&events).iter().map(|e| e.0).collect::<Vec<_>>(),
        [TurnOutcome::Interrupted, TurnOutcome::Completed]
    );
    h.wait("saved", |_| {
        texts(&h.read(id))
            .last()
            .is_some_and(|m| m.1 == "ok, switching")
    })
    .await;
    assert_eq!(
        texts(&h.read(id)),
        vec![
            ("user".into(), "do the first thing".into()),
            ("assistant:Interrupted".into(), "partial".into()),
            ("user".into(), "do the other thing instead".into()),
            ("assistant:Complete".into(), "ok, switching".into()),
        ]
    );
    let s = h.read(id);
    assert!(matches!(
        &s.messages[2].content,
        MessageContent::User {
            delivery: crate::session::UserDelivery::Steering,
            ..
        }
    ));
    let second = chat_bodies(&srv)[1]["messages"].as_array().unwrap().clone();
    let roles: Vec<_> = second.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["system", "user", "assistant", "user"]);
    assert_eq!(second[2]["content"], "partial");
    assert_eq!(second[3]["content"], "do the other thing instead");
}

#[tokio::test]
async fn the_workspace_tools_and_budget_cannot_change_under_a_running_agent() {
    let ws = workspace();
    let other = workspace();
    let (release, hold) = oneshot::channel();
    let srv = fake_agent(vec![chat_route(vec![
        Step::Write(line("x")),
        Step::WaitFor(hold),
        Step::Write(done()),
        Step::Close,
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait("streaming", |_| !h.deltas(id).is_empty()).await;
    h.cmd(ChatCommand::SetWorkspace(id, other.path().to_path_buf()));
    h.cmd(ChatCommand::SetToolEnabled(id, "search_text".into(), false));
    h.cmd(ChatCommand::SetStepBudget(id, 3));
    h.wait("three refusals", |e| {
        e.iter()
            .filter(|e| matches!(e, ChatEvent::SessionError { message, .. } if message.contains("Stop the current run")))
            .count()
            >= 3
    })
    .await;
    release.send(()).unwrap();
    h.wait_run_ended(1).await;
    // Idle again: the same changes now apply.
    h.cmd(ChatCommand::SetWorkspace(id, other.path().to_path_buf()));
    h.cmd(ChatCommand::SetToolEnabled(id, "search_text".into(), false));
    h.wait("applied", |_| {
        let s = h.read(id);
        s.workspace.as_deref() == Some(other.path())
            && s.effective_tools.tools.get("search_text") == Some(&false)
    })
    .await;
}

#[tokio::test]
async fn switched_off_tools_are_not_offered_to_the_model() {
    let ws = workspace();
    let srv = fake_agent(vec![chat_route(reply("done"))]).await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.cmd(ChatCommand::SetToolEnabled(id, "search_text".into(), false));
    h.cmd(ChatCommand::SetCapability(
        id,
        "files.read".into(),
        crate::session::Permission::Auto,
    ));
    h.send(id, "hi").await;
    h.wait_run_ended(1).await;
    let names: Vec<_> = chat_bodies(&srv)[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        [
            "list_dir",
            "read_file",
            "read_binary",
            "find_files",
            "search_bytes",
            "write_file",
            "edit_file",
            "ask_user"
        ]
    );
}

#[tokio::test]
async fn an_agent_needs_a_real_workspace_folder() {
    let srv = fake_agent(vec![]).await;
    let h = start(&srv).await;
    let nowhere = TempDir::new().unwrap().path().join("missing");
    h.cmd(ChatCommand::CreateAgent {
        lifetime: SessionLifetime::Persisted,
        model: "m".into(),
        workspace: Some(nowhere),
    });
    h.cmd(ChatCommand::CreateAgent {
        lifetime: SessionLifetime::Persisted,
        model: "m".into(),
        workspace: Some(std::path::PathBuf::from("relative/dir")),
    });
    h.wait("two refusals", |e| {
        e.iter()
            .filter(|e| matches!(e, ChatEvent::SessionError { session: None, .. }))
            .count()
            >= 2
    })
    .await;
    assert!(h.opened().is_empty());
}

#[tokio::test]
async fn an_incognito_agent_run_leaves_the_data_dir_untouched() {
    let ws = workspace();
    let srv = fake_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("secret findings")),
    ])
    .await;
    let h = start(&srv).await;
    let before = dir_snapshot(h.dir.path());
    let id = h.create_agent(&ws, SessionLifetime::Incognito).await;
    h.send(id, "secret task").await;
    h.wait_run_ended(1).await;
    h.cmd(ChatCommand::ClearIncognito);
    h.handle.shutdown().await;
    let after = dir_snapshot(h.dir.path());
    assert_eq!(after, before);
    for (name, bytes) in &after {
        assert!(!String::from_utf8_lossy(bytes).contains("secret"), "{name}");
    }
}

#[tokio::test]
async fn deleting_a_session_in_the_middle_of_a_tool_run_leaves_nothing_behind() {
    let ws = workspace();
    let (_keep, hold) = oneshot::channel::<()>();
    let srv = fake_agent(vec![chat_route(vec![
        Step::Write(line("x")),
        Step::WaitFor(hold),
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait("streaming", |_| !h.deltas(id).is_empty()).await;
    h.cmd(ChatCommand::Delete(id));
    h.wait("gone", |e| {
        e.iter()
            .rev()
            .take(4)
            .any(|e| matches!(e, ChatEvent::Rows(r) if r.iter().all(|x| x.id != id)))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!h.file(id).exists());
    assert!(run_ends(&h.events()).is_empty());
}

#[tokio::test]
async fn changing_an_idle_agents_settings_reports_them() {
    let ws = workspace();
    let other = workspace();
    let srv = fake_agent(vec![]).await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.cmd(ChatCommand::SetToolEnabled(id, "find_files".into(), false));
    h.cmd(ChatCommand::SetStepBudget(id, 7));
    h.cmd(ChatCommand::SetWorkspace(id, other.path().to_path_buf()));
    h.wait("settings", |e| {
        e.iter().any(|e| {
            matches!(e, ChatEvent::AgentSettings { step_budget: 7, workspace: Some(w), tools, .. }
            if w == other.path() && tools.tools.get("find_files") == Some(&false))
        })
    })
    .await;
}

#[tokio::test]
async fn regenerate_keeps_the_old_reply_as_a_version_and_resends_the_question() {
    let srv = fake(vec![chat_route(reply("one")), chat_route(reply("two"))]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "ask").await;
    h.wait_ended(1).await;
    h.cmd(ChatCommand::Regenerate(id));
    h.wait_ended(2).await;
    h.wait("saved", |_| {
        let s = h.read(id);
        texts(&s)
            == [
                ("user".into(), "ask".into()),
                ("assistant:Complete".into(), "two".into()),
            ]
            && s.forks.len() == 1
    })
    .await;
    let s = h.read(id);
    let new_reply = s.messages.last().unwrap().id;
    assert_eq!(s.version_of(new_reply), Some((2, 2)));
    assert_eq!(texts(&s_with_version(s, 0)).last().unwrap().1, "one");
    let second = &chat_bodies(&srv)[1]["messages"];
    assert_eq!(second.as_array().unwrap().len(), 2, "{second}");
    assert_eq!(second[1]["content"], "ask");
}

/// The saved chat with version `to` of its first fork showing.
fn s_with_version(mut s: Session, to: usize) -> Session {
    let after = s.forks[0].after;
    assert!(s.switch_version(after, to));
    s
}

#[tokio::test]
async fn regenerate_sends_the_window_a_fresh_copy_of_the_chat() {
    let srv = fake(vec![chat_route(reply("one")), chat_route(reply("two"))]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "ask").await;
    h.wait_ended(1).await;
    h.cmd(ChatCommand::Regenerate(id));
    h.wait_ended(2).await;
    let evs = h.events();
    let opened = evs
        .iter()
        .rposition(
            |e| matches!(e, ChatEvent::Opened(s) if s.forks.len() == 1 && s.messages.len() == 2),
        )
        .expect("Opened with the new version");
    let started = evs
        .iter()
        .rposition(|e| matches!(e, ChatEvent::TurnStarted { .. }))
        .unwrap();
    assert!(
        opened < started,
        "snapshot {opened} after turn start {started}"
    );
}

#[tokio::test]
async fn editing_a_message_makes_a_new_version_and_asks_again_from_there() {
    let srv = fake(vec![chat_route(reply("one")), chat_route(reply("two"))]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "ask").await;
    h.wait_ended(1).await;
    let first = h.read(id).messages[0].id;
    h.cmd(ChatCommand::Edit {
        session: id,
        message: first,
        text: "better ask".into(),
    });
    h.wait_ended(2).await;
    h.wait("saved", |_| {
        let s = h.read(id);
        texts(&s).len() == 2 && texts(&s)[1].1 == "two" && s.forks.len() == 1
    })
    .await;
    let s = h.read(id);
    assert_eq!(texts(&s)[0].1, "better ask");
    assert_eq!(s.version_of(s.messages[0].id), Some((2, 2)));
    let old = s_with_version(s, 0);
    assert_eq!(texts(&old)[0].1, "ask");
    assert_eq!(texts(&old)[1].1, "one");
    let second = &chat_bodies(&srv)[1]["messages"];
    assert_eq!(second.as_array().unwrap().len(), 2, "{second}");
    assert_eq!(second[1]["content"], "better ask");
}

#[tokio::test]
async fn an_edit_of_anything_but_your_own_message_is_refused() {
    let srv = fake(vec![chat_route(reply("one"))]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "ask").await;
    h.wait_ended(1).await;
    let reply_id = h.read(id).messages[1].id;
    for (message, text) in [
        (reply_id, "x"),
        (crate::session::MessageId::new(), "x"),
        (h.read(id).messages[0].id, "   "),
    ] {
        h.cmd(ChatCommand::Edit {
            session: id,
            message,
            text: text.into(),
        });
    }
    h.wait("refused", |evs| {
        evs.iter()
            .filter(|e| {
                matches!(
                    e,
                    ChatEvent::SessionError { .. } | ChatEvent::SendRejected { .. }
                )
            })
            .count()
            >= 3
    })
    .await;
    assert_eq!(chat_bodies(&srv).len(), 1, "no new request");
    assert!(h.read(id).forks.is_empty());
}

#[tokio::test]
async fn switching_versions_changes_what_is_shown_and_saved() {
    let srv = fake(vec![chat_route(reply("one")), chat_route(reply("two"))]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "ask").await;
    h.wait_ended(1).await;
    h.cmd(ChatCommand::Regenerate(id));
    h.wait_ended(2).await;
    h.wait("saved", |_| h.read(id).forks.len() == 1).await;
    let user = h.read(id).messages[0].id;
    h.cmd(ChatCommand::SwitchVersion {
        session: id,
        after: Some(user),
        to: 0,
    });
    let shows_one = |evs: &[ChatEvent]| {
        evs.iter().rev().find_map(|e| match e {
            ChatEvent::Opened(s) => Some(texts(s)[1].1 == "one"),
            _ => None,
        }) == Some(true)
    };
    h.wait("window told", shows_one).await;
    h.wait("saved", |_| texts(&h.read(id))[1].1 == "one").await;
}

#[tokio::test]
async fn regenerate_does_nothing_when_the_last_reply_did_not_complete() {
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("half")),
        Step::Close,
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "ask").await;
    h.wait_ended(1).await;
    h.cmd(ChatCommand::Regenerate(id));
    h.wait("refused", |evs| {
        evs.iter()
            .any(|e| matches!(e, ChatEvent::SessionError { .. }))
    })
    .await;
    assert_eq!(chat_bodies(&srv).len(), 1, "no second request");
    assert_eq!(texts(&h.read(id)).len(), 2, "the failed reply is kept");
}

#[tokio::test]
async fn a_finished_reply_keeps_the_numbers_ollama_reported() {
    let done_with_stats = format!(
        "{}\n",
        serde_json::json!({
            "message": {"role": "assistant", "content": ""},
            "done": true, "done_reason": "stop",
            "prompt_eval_count": 12, "eval_count": 40,
            "total_duration": 2_500_000_000u64,
            "load_duration": 300_000_000u64,
            "eval_duration": 2_000_000_000u64
        })
    )
    .into_bytes();
    let srv = fake(vec![chat_route(vec![
        Step::Write(line("hi")),
        Step::Write(done_with_stats),
        Step::Close,
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "ask").await;
    h.wait_ended(1).await;
    h.wait("saved", |_| {
        h.read(id).messages.iter().any(|m| {
            matches!(&m.content, MessageContent::Assistant { stats: Some(s), .. }
                if s.prompt_tokens == Some(12) && s.reply_tokens == Some(40)
                && s.total_ms == Some(2_500) && s.load_ms == Some(300) && s.eval_ms == Some(2_000))
        })
    })
    .await;
}

async fn chat_with_one_exchange(h: &H) -> (SessionId, MessageId) {
    let id = h.create(SessionLifetime::Persisted).await;
    h.send(id, "ask").await;
    h.wait_ended(1).await;
    h.wait("saved", |_| {
        h.try_read(id).is_some_and(|s| s.messages.len() == 2)
    })
    .await;
    (id, h.read(id).messages[1].id)
}

#[tokio::test]
async fn branching_creates_a_new_chat_with_the_history_and_opens_it() {
    let srv = fake(vec![chat_route(reply("one"))]).await;
    let h = start(&srv).await;
    let (id, at) = chat_with_one_exchange(&h).await;
    h.cmd(ChatCommand::Branch {
        session: id,
        message: at,
        model: Some("other:7b".into()),
    });
    h.wait("opened", |evs| {
        evs.iter()
            .any(|e| matches!(e, ChatEvent::Opened(s) if s.id != id && s.model == "other:7b"))
    })
    .await;
    let new_id = h
        .events()
        .iter()
        .rev()
        .find_map(|e| match e {
            ChatEvent::Opened(s) if s.id != id => Some(s.id),
            _ => None,
        })
        .unwrap();
    h.wait("saved", |_| h.read(new_id).messages.len() == 2)
        .await;
    let b = h.read(new_id);
    assert_eq!(
        b.parent.map(|p| (p.session_id, p.message_id)),
        Some((id, at))
    );
    assert_eq!(b.lineage.len(), 1);
    assert_eq!(h.read(id).messages.len(), 2, "the original is untouched");
}

#[tokio::test]
async fn branching_from_an_unfinished_reply_is_refused() {
    let srv = fake(vec![chat_route(reply("one"))]).await;
    let h = start(&srv).await;
    let (id, _) = chat_with_one_exchange(&h).await;
    let user = h.read(id).messages[0].id;
    h.cmd(ChatCommand::Branch {
        session: id,
        message: user,
        model: None,
    });
    h.wait("refused", |evs| {
        evs.iter()
            .any(|e| matches!(e, ChatEvent::SessionError { .. }))
    })
    .await;
    assert!(!h
        .events()
        .iter()
        .any(|e| matches!(e, ChatEvent::Opened(s) if s.id != id)));
}

async fn branch_and_open(h: &H, id: SessionId, at: MessageId) -> SessionId {
    h.cmd(ChatCommand::Branch {
        session: id,
        message: at,
        model: None,
    });
    h.wait("opened", |evs| {
        evs.iter()
            .any(|e| matches!(e, ChatEvent::Opened(s) if s.id != id))
    })
    .await;
    let new_id = h
        .events()
        .iter()
        .rev()
        .find_map(|e| match e {
            ChatEvent::Opened(s) if s.id != id => Some(s.id),
            _ => None,
        })
        .unwrap();
    h.wait("saved", |_| h.read(new_id).messages.len() == 2)
        .await;
    new_id
}

fn last_branch_state(h: &H, session: SessionId) -> Option<BranchState> {
    h.events().iter().rev().find_map(|e| match e {
        ChatEvent::BranchChecked { session: s, state } if *s == session => Some(*state),
        _ => None,
    })
}

#[tokio::test]
async fn a_branch_reports_whether_its_original_changed_or_is_gone() {
    let srv = fake(vec![chat_route(reply("one")), chat_route(reply("two"))]).await;
    let h = start(&srv).await;
    let (id, at) = chat_with_one_exchange(&h).await;
    let b = branch_and_open(&h, id, at).await;

    h.cmd(ChatCommand::CheckBranch(b));
    h.wait("unchanged", |_| {
        last_branch_state(&h, b) == Some(BranchState::Unchanged)
    })
    .await;

    // Regenerating the branch point's reply changes the original's history.
    h.cmd(ChatCommand::Regenerate(id));
    h.wait_ended(2).await;
    h.wait("saved", |_| h.read(id).forks.len() == 1).await;
    h.cmd(ChatCommand::CheckBranch(b));
    h.wait("changed", |_| {
        last_branch_state(&h, b) == Some(BranchState::Changed)
    })
    .await;

    // Switching back makes it match again.
    let user = h.read(id).messages[0].id;
    h.cmd(ChatCommand::SwitchVersion {
        session: id,
        after: Some(user),
        to: 0,
    });
    h.wait("saved back", |_| h.read(id).messages[1].id == at)
        .await;
    h.cmd(ChatCommand::CheckBranch(b));
    h.wait("unchanged again", |evs| {
        evs.iter()
            .filter(|e| {
                matches!(
                    e,
                    ChatEvent::BranchChecked {
                        state: BranchState::Unchanged,
                        ..
                    }
                )
            })
            .count()
            >= 2
    })
    .await;

    h.cmd(ChatCommand::Delete(id));
    h.wait("deleted", |_| {
        h.cmd(ChatCommand::CheckBranch(b));
        last_branch_state(&h, b) == Some(BranchState::Deleted)
    })
    .await;
}

fn btw_done(h: &H, session: SessionId) -> Option<crate::session::BtwEntry> {
    h.events().iter().rev().find_map(|e| match e {
        ChatEvent::BtwDone { session: s, entry } if *s == session => Some(entry.clone()),
        _ => None,
    })
}

#[tokio::test]
async fn a_side_question_is_answered_from_the_history_and_stays_out_of_the_chat() {
    let srv = fake(vec![chat_route(reply("one")), chat_route(reply("side"))]).await;
    let h = start(&srv).await;
    let (id, at) = chat_with_one_exchange(&h).await;
    h.cmd(ChatCommand::Btw {
        session: id,
        message: at,
        question: "why?".into(),
    });
    h.wait("answered", |_| btw_done(&h, id).is_some()).await;
    let entry = btw_done(&h, id).unwrap();
    assert_eq!(
        (entry.question.as_str(), entry.answer.as_str()),
        ("why?", "side")
    );
    let body = &chat_bodies(&srv)[1]["messages"];
    let roles: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["system", "user", "assistant", "user"]);
    assert_eq!(body[3]["content"], "why?");
    h.wait("saved", |_| {
        h.try_read(id).is_some_and(|s| s.btw.len() == 1)
    })
    .await;
    let s = h.read(id);
    assert_eq!(s.messages.len(), 2, "the transcript is untouched");
}

#[tokio::test]
async fn a_side_question_that_fails_says_so_and_is_not_kept() {
    let srv = fake(vec![
        chat_route(reply("one")),
        chat_route(vec![Step::Close]),
    ])
    .await;
    let h = start(&srv).await;
    let (id, at) = chat_with_one_exchange(&h).await;
    h.cmd(ChatCommand::Btw {
        session: id,
        message: at,
        question: "why?".into(),
    });
    h.wait("failed", |evs| {
        evs.iter()
            .any(|e| matches!(e, ChatEvent::BtwFailed { session, .. } if *session == id))
    })
    .await;
    assert!(h.read(id).btw.is_empty());
}

#[tokio::test]
async fn adding_a_side_question_appends_it_and_deleting_forgets_it() {
    let srv = fake(vec![chat_route(reply("one")), chat_route(reply("side"))]).await;
    let h = start(&srv).await;
    let (id, at) = chat_with_one_exchange(&h).await;
    h.cmd(ChatCommand::Btw {
        session: id,
        message: at,
        question: "why?".into(),
    });
    h.wait("answered", |_| btw_done(&h, id).is_some()).await;
    let entry = btw_done(&h, id).unwrap();
    h.cmd(ChatCommand::BtwAdd {
        session: id,
        id: entry.id,
    });
    h.wait("added", |_| {
        h.try_read(id).is_some_and(|s| s.messages.len() == 4)
    })
    .await;
    let s = h.read(id);
    assert_eq!(texts(&s)[3], ("assistant:Complete".into(), "side".into()));
    assert!(s.btw[0].added);
    h.cmd(ChatCommand::BtwDelete {
        session: id,
        id: entry.id,
    });
    h.wait("deleted", |_| {
        h.try_read(id).is_some_and(|s| s.btw.is_empty())
    })
    .await;
    assert_eq!(
        h.read(id).messages.len(),
        4,
        "deleting does not undo the add"
    );
}

// ---- tool approval

fn approval_for(e: &[ChatEvent]) -> Option<(String, String)> {
    e.iter().rev().find_map(|e| match e {
        ChatEvent::ApprovalRequested { call_id, name, .. } => Some((call_id.clone(), name.clone())),
        _ => None,
    })
}

fn count_approvals(e: &[ChatEvent]) -> usize {
    e.iter()
        .filter(|e| matches!(e, ChatEvent::ApprovalRequested { .. }))
        .count()
}

fn started_tools(e: &[ChatEvent]) -> usize {
    e.iter()
        .filter(|e| matches!(e, ChatEvent::ToolStarted { .. }))
        .count()
}

/// An agent that asks before reading files, working on a=hello.
async fn asking_agent(routes: Vec<Route>) -> (TcpScript, H, SessionId, TempDir) {
    let ws = workspace();
    let srv = fake_agent(routes).await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.cmd(ChatCommand::SetCapability(
        id,
        "files.read".into(),
        crate::session::Permission::Ask,
    ));
    h.wait("capability set", |_| {
        h.try_read(id).is_some_and(|s| {
            s.effective_tools.capabilities.get("files.read")
                == Some(&crate::session::Permission::Ask)
        })
    })
    .await;
    (srv, h, id, ws)
}

#[tokio::test]
async fn a_tool_set_to_ask_waits_for_the_user_and_runs_once_allowed() {
    let (srv, h, id, _ws) = asking_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("It says hello.")),
    ])
    .await;
    h.send(id, "what is in a.txt?").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    // The model was offered the tools even though they need approval.
    assert_eq!(chat_bodies(&srv)[0]["tools"].as_array().unwrap().len(), 9);
    let (call_id, name) = approval_for(&h.events()).unwrap();
    assert_eq!(name, "read_file");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        started_tools(&h.events()),
        0,
        "nothing runs before the answer"
    );
    assert!(run_ends(&h.events()).is_empty(), "the run waits");

    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Allow,
    });
    h.wait_run_ended(1).await;
    let events = h.events();
    assert_eq!(started_tools(&events), 1);
    assert!(events.iter().any(|e| matches!(e, ChatEvent::ToolFinished { ok: true, output, .. } if output == "1| hello\n2| world")));
    assert_eq!(
        run_ends(&events),
        vec![(super::types::RunOutcome::Completed, None)]
    );
}

// ---- leaving an agent chat does not end its run

#[tokio::test]
async fn leaving_an_agent_that_waits_for_approval_keeps_the_run_and_the_question() {
    let (_srv, h, id, _ws) = asking_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("It says hello.")),
    ])
    .await;
    h.send(id, "what is in a.txt?").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    // The user opens another chat: the window lets this one go.
    h.cmd(ChatCommand::Close(id));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(run_ends(&h.events()).is_empty(), "the run is still waiting");
    // Coming back, the question is asked again and the run goes on.
    h.cmd(ChatCommand::Open(id));
    h.wait("asked again", |e| count_approvals(e) >= 2).await;
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Allow,
    });
    h.wait_run_ended(1).await;
    assert_eq!(
        run_ends(&h.events()),
        vec![(super::types::RunOutcome::Completed, None)]
    );
}

#[tokio::test]
async fn an_approval_can_be_answered_after_the_window_let_the_chat_go() {
    let (_srv, h, id, _ws) = asking_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("It says hello.")),
    ])
    .await;
    h.send(id, "what is in a.txt?").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::Close(id));
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Allow,
    });
    h.wait_run_ended(1).await;
    assert_eq!(
        run_ends(&h.events()),
        vec![(super::types::RunOutcome::Completed, None)]
    );
}

#[tokio::test]
async fn leaving_a_streaming_agent_lets_the_whole_run_finish() {
    let (hold, release) = oneshot::channel::<()>();
    let mut first = vec![Step::WaitFor(release)];
    first.extend(calls("read_file", serde_json::json!({"path": "a.txt"})));
    let ws = workspace();
    let srv = fake_agent(vec![chat_route(first), chat_route(reply("It says hello."))]).await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.send(id, "what is in a.txt?").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.cmd(ChatCommand::Close(id));
    hold.send(()).unwrap();
    h.wait_run_ended(1).await;
    assert_eq!(
        run_ends(&h.events()),
        vec![(super::types::RunOutcome::Completed, None)]
    );
    h.wait("the finished run is saved", |_| {
        h.try_read(id)
            .is_some_and(|s| texts(&s).iter().any(|m| m.1 == "It says hello."))
    })
    .await;
}

// ---- the agent asks the user a question

use crate::agent::ask::{Answer, Question};

fn question_for(e: &[ChatEvent]) -> Option<(String, Question)> {
    e.iter().rev().find_map(|e| match e {
        ChatEvent::QuestionAsked {
            call_id, question, ..
        } => Some((call_id.clone(), question.clone())),
        _ => None,
    })
}

fn count_questions(e: &[ChatEvent]) -> usize {
    e.iter()
        .filter(|e| matches!(e, ChatEvent::QuestionAsked { .. }))
        .count()
}

fn one_of(options: &[&str]) -> serde_json::Value {
    serde_json::json!({"question": "Which one?", "kind": "one", "options": options})
}

fn picked(p: &[&str]) -> Option<Answer> {
    Some(Answer {
        picked: p.iter().map(|s| s.to_string()).collect(),
        other: None,
    })
}

fn typed(t: &str) -> Option<Answer> {
    Some(Answer {
        picked: Vec::new(),
        other: Some(t.into()),
    })
}

/// An agent in a plain folder whose model asks `ask` first, then answers.
async fn asking_user_agent(asks: Vec<serde_json::Value>) -> (TcpScript, H, SessionId, TempDir) {
    let ws = workspace();
    let mut routes: Vec<Route> = asks
        .into_iter()
        .map(|a| chat_route(calls("ask_user", a)))
        .collect();
    routes.push(chat_route(reply("Thanks, noted.")));
    let srv = fake_agent(routes).await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    (srv, h, id, ws)
}

/// What the model was told after the user answered the question.
fn tool_reply(srv: &TcpScript, n: usize) -> String {
    let m = chat_bodies(srv)[n]["messages"].clone();
    let last = m.as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["role"], "tool", "{last}");
    last["content"].as_str().unwrap().to_string()
}

async fn ask_and_answer(args: serde_json::Value, answer: Option<Answer>) -> String {
    let (srv, h, id, _ws) = asking_user_agent(vec![args]).await;
    h.send(id, "help me choose").await;
    h.wait("asked", |e| question_for(e).is_some()).await;
    let (call_id, _) = question_for(&h.events()).unwrap();
    h.cmd(ChatCommand::AnswerQuestion {
        session: id,
        call_id,
        answer,
    });
    h.wait_run_ended(1).await;
    assert_eq!(
        run_ends(&h.events()),
        vec![(super::types::RunOutcome::Completed, None)]
    );
    tool_reply(&srv, 1)
}

#[tokio::test]
async fn the_agent_asks_and_waits_and_the_answer_reaches_the_model() {
    let (srv, h, id, _ws) = asking_user_agent(vec![one_of(&["A", "B"])]).await;
    h.send(id, "help me choose").await;
    h.wait("asked", |e| question_for(e).is_some()).await;
    let (call_id, q) = question_for(&h.events()).unwrap();
    assert_eq!(q.text, "Which one?");
    assert_eq!(q.options, ["A", "B"]);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(run_ends(&h.events()).is_empty(), "the run waits");
    assert_eq!(started_tools(&h.events()), 0, "nothing ran");
    assert_eq!(
        count_approvals(&h.events()),
        0,
        "a question is not an approval"
    );
    h.cmd(ChatCommand::AnswerQuestion {
        session: id,
        call_id,
        answer: picked(&["B"]),
    });
    h.wait_run_ended(1).await;
    assert_eq!(tool_reply(&srv, 1), r#"{"answer":"B"}"#);
}

#[tokio::test]
async fn each_kind_of_answer_is_passed_on() {
    assert_eq!(
        ask_and_answer(one_of(&["A", "B"]), typed("Neither, use C")).await,
        r#"{"answer":"Neither, use C","other":true}"#
    );
    let many =
        serde_json::json!({"question": "Which?", "kind": "many", "options": ["A", "B", "C"]});
    assert_eq!(
        ask_and_answer(many, picked(&["A", "C"])).await,
        r#"{"answers":["A","C"]}"#
    );
    let text = serde_json::json!({"question": "Name?", "kind": "text"});
    assert_eq!(
        ask_and_answer(text, typed("Ada")).await,
        r#"{"answer":"Ada"}"#
    );
}

#[tokio::test]
async fn skipping_tells_the_model_and_the_run_goes_on() {
    let r = ask_and_answer(one_of(&["A", "B"]), None).await;
    assert!(r.contains("chose not to answer"), "{r}");
}

#[tokio::test]
async fn a_malformed_question_goes_back_to_the_model_and_does_not_bother_the_user() {
    let (srv, h, id, _ws) = asking_user_agent(vec![
        serde_json::json!({"question": "Which?", "kind": "one"}),
    ])
    .await;
    h.send(id, "help me choose").await;
    h.wait_run_ended(1).await;
    assert_eq!(count_questions(&h.events()), 0);
    let r = tool_reply(&srv, 1);
    assert!(
        r.contains("invalid arguments") && r.contains("2 to 8"),
        "{r}"
    );
}

#[tokio::test]
async fn an_answer_that_is_not_one_of_the_options_is_asked_again() {
    let (srv, h, id, _ws) = asking_user_agent(vec![one_of(&["A", "B"])]).await;
    h.send(id, "help me choose").await;
    h.wait("asked", |e| question_for(e).is_some()).await;
    let (call_id, _) = question_for(&h.events()).unwrap();
    h.cmd(ChatCommand::AnswerQuestion {
        session: id,
        call_id: call_id.clone(),
        answer: picked(&["Z"]),
    });
    h.wait("asked again", |e| count_questions(e) >= 2).await;
    assert!(run_ends(&h.events()).is_empty(), "still waiting");
    h.cmd(ChatCommand::AnswerQuestion {
        session: id,
        call_id,
        answer: picked(&["A"]),
    });
    h.wait_run_ended(1).await;
    assert_eq!(tool_reply(&srv, 1), r#"{"answer":"A"}"#);
}

#[tokio::test]
async fn leaving_the_chat_keeps_the_question_and_coming_back_asks_it_again() {
    let (srv, h, id, _ws) = asking_user_agent(vec![one_of(&["A", "B"])]).await;
    h.send(id, "help me choose").await;
    h.wait("asked", |e| question_for(e).is_some()).await;
    h.cmd(ChatCommand::Close(id));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(run_ends(&h.events()).is_empty(), "the run is still waiting");
    h.cmd(ChatCommand::Open(id));
    h.wait("asked again", |e| count_questions(e) >= 2).await;
    assert_eq!(count_approvals(&h.events()), 0);
    let (call_id, _) = question_for(&h.events()).unwrap();
    h.cmd(ChatCommand::AnswerQuestion {
        session: id,
        call_id,
        answer: picked(&["B"]),
    });
    h.wait_run_ended(1).await;
    assert_eq!(tool_reply(&srv, 1), r#"{"answer":"B"}"#);
}

#[tokio::test]
async fn stop_ends_a_run_that_is_waiting_for_an_answer() {
    let (_srv, h, id, _ws) = asking_user_agent(vec![one_of(&["A", "B"])]).await;
    h.send(id, "help me choose").await;
    h.wait("asked", |e| question_for(e).is_some()).await;
    h.cmd(ChatCommand::Stop(id));
    h.wait_run_ended(1).await;
    assert_eq!(
        run_ends(&h.events()),
        vec![(super::types::RunOutcome::Cancelled, None)]
    );
}

#[tokio::test]
async fn the_question_and_the_answer_are_kept_in_the_chat() {
    let (_srv, h, id, _ws) = asking_user_agent(vec![one_of(&["A", "B"])]).await;
    h.send(id, "help me choose").await;
    h.wait("asked", |e| question_for(e).is_some()).await;
    let (call_id, _) = question_for(&h.events()).unwrap();
    h.cmd(ChatCommand::AnswerQuestion {
        session: id,
        call_id,
        answer: picked(&["B"]),
    });
    h.wait_run_ended(1).await;
    h.wait("saved", |_| {
        h.try_read(id).is_some_and(|s| {
            s.messages.iter().any(|m| {
                matches!(&m.content, crate::session::MessageContent::Tool { name, output, status, .. }
                    if name == "ask_user"
                        && output.as_deref() == Some(r#"{"answer":"B"}"#)
                        && *status == crate::session::ToolStatus::Complete)
            })
        })
    })
    .await;
}

#[tokio::test]
async fn a_run_may_ask_only_so_many_questions() {
    let asks: Vec<_> = (0..6).map(|_| one_of(&["A", "B"])).collect();
    let (srv, h, id, _ws) = asking_user_agent(asks).await;
    h.send(id, "ask a lot").await;
    for n in 1..=5 {
        h.wait("asked", |e| count_questions(e) >= n).await;
        let (call_id, _) = question_for(&h.events()).unwrap();
        h.cmd(ChatCommand::AnswerQuestion {
            session: id,
            call_id,
            answer: None,
        });
    }
    h.wait_run_ended(1).await;
    assert_eq!(count_questions(&h.events()), 5, "the sixth is not asked");
    let r = tool_reply(&srv, 6);
    assert!(r.contains("5 questions"), "{r}");
}

#[tokio::test]
async fn the_model_is_offered_the_tool_unless_it_is_switched_off() {
    let names = |srv: &TcpScript| -> Vec<String> {
        chat_bodies(srv)[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_string())
            .collect()
    };
    let (srv, h, id, _ws) = asking_user_agent(vec![]).await;
    h.send(id, "hello").await;
    h.wait_run_ended(1).await;
    assert!(names(&srv).contains(&"ask_user".to_string()));

    let (srv, h, id, _ws) = asking_user_agent(vec![]).await;
    h.cmd(ChatCommand::SetToolEnabled(id, "ask_user".into(), false));
    h.wait("off", |_| {
        h.try_read(id)
            .is_some_and(|s| s.effective_tools.tools.get("ask_user") == Some(&false))
    })
    .await;
    h.send(id, "hello").await;
    h.wait_run_ended(1).await;
    assert!(!names(&srv).contains(&"ask_user".to_string()));
}

#[tokio::test]
async fn a_declined_tool_call_is_reported_to_the_model_and_the_run_goes_on() {
    let (srv, h, id, _ws) = asking_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("Understood, I will not read it.")),
    ])
    .await;
    h.send(id, "what is in a.txt?").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Deny,
    });
    h.wait_run_ended(1).await;
    let events = h.events();
    assert_eq!(started_tools(&events), 0, "the tool never ran");
    assert!(events.iter().any(|e| matches!(e, ChatEvent::ToolFinished { ok: false, output, .. } if output.contains("declined"))));
    let second = chat_bodies(&srv)[1]["messages"].clone();
    let last = second.as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["role"], "tool");
    assert!(
        last["content"].as_str().unwrap().contains("declined"),
        "{last}"
    );
}

#[tokio::test]
async fn allow_for_this_session_stops_further_questions() {
    let (_srv, h, id, _ws) = asking_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("Done.")),
    ])
    .await;
    h.send(id, "read it twice").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::AllowForSession,
    });
    h.wait_run_ended(1).await;
    let events = h.events();
    assert_eq!(count_approvals(&events), 1, "asked once, not twice");
    assert_eq!(started_tools(&events), 2);
    h.wait("saved", |_| {
        h.try_read(id).is_some_and(|s| {
            s.effective_tools.capabilities.get("files.read")
                == Some(&crate::session::Permission::Auto)
        })
    })
    .await;
}

fn preview_of(e: &[ChatEvent]) -> Option<crate::agent::change::ChangePreview> {
    e.iter().rev().find_map(|e| match e {
        ChatEvent::ApprovalRequested { preview, .. } => preview.clone(),
        _ => None,
    })
}

#[tokio::test]
async fn a_write_waits_with_a_diff_and_happens_only_once_allowed() {
    let (_srv, h, id, ws) = asking_agent(vec![
        chat_route(calls(
            "write_file",
            serde_json::json!({"path": "notes/new.txt", "content": "hi\nthere\n"}),
        )),
        chat_route(reply("Done.")),
    ])
    .await;
    h.send(id, "write a note").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    let p = preview_of(&h.events()).expect("a write shows what it would change");
    assert_eq!(p.path, "notes/new.txt");
    assert!(p.created);
    assert_eq!((p.added, p.removed), (2, 0));
    assert!(
        !ws.path().join("notes/new.txt").exists(),
        "nothing before the answer"
    );
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Allow,
    });
    h.wait_run_ended(1).await;
    assert_eq!(
        std::fs::read_to_string(ws.path().join("notes/new.txt")).unwrap(),
        "hi\nthere\n"
    );
}

#[tokio::test]
async fn with_writing_switched_off_in_settings_no_agent_is_offered_or_can_use_a_write() {
    let (srv, h, id, ws) = asking_agent(vec![
        chat_route(calls(
            "write_file",
            serde_json::json!({"path": "sneaky.txt", "content": "x\n"}),
        )),
        chat_route(reply("Could not.")),
    ])
    .await;
    h.cmd(ChatCommand::SetWritesAllowed(false));
    h.send(id, "write something").await;
    h.wait_run_ended(1).await;
    assert!(!ws.path().join("sneaky.txt").exists());
    assert_eq!(count_approvals(&h.events()), 0, "refused, not even asked");
    let names: Vec<_> = chat_bodies(&srv)[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        [
            "list_dir",
            "read_file",
            "read_binary",
            "find_files",
            "search_text",
            "search_bytes",
            "ask_user"
        ]
    );
}

fn changes_of(e: &[ChatEvent]) -> Vec<crate::session::ChangeRecord> {
    e.iter()
        .rev()
        .find_map(|e| match e {
            ChatEvent::Changes { changes, .. } => Some(changes.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn allow_next(h: &H, id: SessionId) {
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Allow,
    });
}

/// An agent that overwrites a.txt once and is allowed to.
async fn agent_that_changed_a_file() -> (TcpScript, H, SessionId, TempDir) {
    let (srv, h, id, ws) = asking_agent(vec![
        chat_route(calls(
            "write_file",
            serde_json::json!({"path": "a.txt", "content": "changed\n"}),
        )),
        chat_route(reply("Done.")),
    ])
    .await;
    h.send(id, "change a.txt").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    allow_next(&h, id);
    h.wait_run_ended(1).await;
    (srv, h, id, ws)
}

#[tokio::test]
async fn a_change_is_recorded_with_its_original_and_saved_with_the_chat() {
    let (_srv, h, id, ws) = agent_that_changed_a_file().await;
    let c = changes_of(&h.events());
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].path, "a.txt");
    assert_eq!(c[0].before, "hello\nworld\n");
    assert!(!c[0].created && !c[0].undone);
    assert_eq!((c[0].added, c[0].removed), (1, 2));
    assert_eq!(
        std::fs::read_to_string(ws.path().join("a.txt")).unwrap(),
        "changed\n"
    );
    h.wait("saved", |_| {
        h.try_read(id).is_some_and(|s| s.changes.len() == 1)
    })
    .await;
}

#[tokio::test]
async fn undo_puts_the_original_back_and_marks_the_change() {
    let (_srv, h, id, ws) = agent_that_changed_a_file().await;
    h.cmd(ChatCommand::UndoChange {
        session: id,
        change: 1,
    });
    h.wait("undone", |e| {
        changes_of(e).first().is_some_and(|c| c.undone)
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(ws.path().join("a.txt")).unwrap(),
        "hello\nworld\n"
    );
    let c = changes_of(&h.events());
    assert!(
        c[0].before.is_empty(),
        "the copy is dropped once it is not needed"
    );
}

#[tokio::test]
async fn undo_refuses_when_the_file_was_edited_since_and_leaves_it_alone() {
    let (_srv, h, id, ws) = agent_that_changed_a_file().await;
    std::fs::write(ws.path().join("a.txt"), "my own edit\n").unwrap();
    h.cmd(ChatCommand::UndoChange {
        session: id,
        change: 1,
    });
    h.wait("refused", |e| {
        e.iter()
            .any(|e| matches!(e, ChatEvent::UndoRefused { change: 1, .. }))
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(ws.path().join("a.txt")).unwrap(),
        "my own edit\n"
    );
    assert!(!changes_of(&h.events())[0].undone);
}

#[tokio::test]
async fn undo_refuses_after_the_agent_moved_to_another_folder() {
    let (_srv, h, id, ws) = agent_that_changed_a_file().await;
    let other = workspace();
    // Same relative file, same content as the agent wrote, in the new folder.
    std::fs::write(other.path().join("a.txt"), "changed\n").unwrap();
    h.cmd(ChatCommand::SetWorkspace(id, other.path().to_path_buf()));
    h.wait("moved", |_| {
        h.try_read(id)
            .is_some_and(|s| s.workspace.as_deref() == Some(other.path()))
    })
    .await;
    h.cmd(ChatCommand::UndoChange {
        session: id,
        change: 1,
    });
    h.wait("refused", |e| {
        e.iter()
            .any(|e| matches!(e, ChatEvent::UndoRefused { change: 1, .. }))
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(other.path().join("a.txt")).unwrap(),
        "changed\n"
    );
    assert_eq!(
        std::fs::read_to_string(ws.path().join("a.txt")).unwrap(),
        "changed\n"
    );
}

#[tokio::test]
async fn undoing_a_created_file_removes_it() {
    let (_srv, h, id, ws) = asking_agent(vec![
        chat_route(calls(
            "write_file",
            serde_json::json!({"path": "new/n.txt", "content": "hi\n"}),
        )),
        chat_route(reply("Done.")),
    ])
    .await;
    h.send(id, "make a file").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    allow_next(&h, id);
    h.wait_run_ended(1).await;
    assert!(changes_of(&h.events())[0].created);
    h.cmd(ChatCommand::UndoChange {
        session: id,
        change: 1,
    });
    h.wait("undone", |e| {
        changes_of(e).first().is_some_and(|c| c.undone)
    })
    .await;
    assert!(!ws.path().join("new/n.txt").exists());
}

#[tokio::test]
async fn undo_all_goes_newest_first_and_stops_at_a_file_that_changed() {
    let (_srv, h, id, ws) = asking_agent(vec![
        chat_route(calls(
            "write_file",
            serde_json::json!({"path": "a.txt", "content": "one\n"}),
        )),
        chat_route(calls(
            "write_file",
            serde_json::json!({"path": "a.txt", "content": "two\n"}),
        )),
        chat_route(reply("Done.")),
    ])
    .await;
    h.send(id, "change a.txt twice").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    // Allowing this file for the session covers the second write too.
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::AllowForSession,
    });
    h.wait_run_ended(1).await;
    assert_eq!(changes_of(&h.events()).len(), 2);
    h.cmd(ChatCommand::UndoAll(id));
    h.wait("all undone", |e| {
        changes_of(e).iter().all(|c| c.undone) && !changes_of(e).is_empty()
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(ws.path().join("a.txt")).unwrap(),
        "hello\nworld\n"
    );
}

#[tokio::test]
async fn nothing_can_be_undone_while_a_run_is_active() {
    let (_srv, h, id, _ws) = asking_agent(vec![chat_route(calls(
        "write_file",
        serde_json::json!({"path": "a.txt", "content": "x\n"}),
    ))])
    .await;
    h.send(id, "change it").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    h.cmd(ChatCommand::UndoAll(id));
    h.wait("refused", |e| {
        e.iter().any(|e| matches!(e, ChatEvent::UndoRefused { .. }))
    })
    .await;
}

#[tokio::test]
async fn a_write_set_to_allow_runs_without_asking_and_can_still_be_undone() {
    let (_srv, h, id, ws) = asking_agent(vec![
        chat_route(calls(
            "write_file",
            serde_json::json!({"path": "auto.txt", "content": "yes\n"}),
        )),
        chat_route(reply("Done.")),
    ])
    .await;
    h.cmd(ChatCommand::SetCapability(
        id,
        "files.write".into(),
        crate::session::Permission::Auto,
    ));
    h.wait("write allowed", |_| {
        h.try_read(id).is_some_and(|s| {
            s.effective_tools.capabilities.get("files.write")
                == Some(&crate::session::Permission::Auto)
        })
    })
    .await;
    h.send(id, "make it").await;
    h.wait_run_ended(1).await;
    assert_eq!(count_approvals(&h.events()), 0);
    assert_eq!(
        std::fs::read_to_string(ws.path().join("auto.txt")).unwrap(),
        "yes\n"
    );
    assert_eq!(changes_of(&h.events()).len(), 1);
}

/// An agent that may run commands (the Settings switch is on) and asks first.
async fn commanding_agent(routes: Vec<Route>) -> (TcpScript, H, SessionId, TempDir) {
    let (srv, h, id, ws) = asking_agent(routes).await;
    h.cmd(ChatCommand::SetRunAllowed(true));
    h.cmd(ChatCommand::SetCapability(
        id,
        "shell.run".into(),
        crate::session::Permission::Ask,
    ));
    h.wait("run set to ask", |_| {
        h.try_read(id).is_some_and(|s| {
            s.effective_tools.capabilities.get("shell.run")
                == Some(&crate::session::Permission::Ask)
        })
    })
    .await;
    (srv, h, id, ws)
}

fn run_preview_of(e: &[ChatEvent]) -> Option<crate::agent::command::RunPreview> {
    e.iter().rev().find_map(|e| match e {
        ChatEvent::ApprovalRequested { run, .. } => run.clone(),
        _ => None,
    })
}

fn tool_outputs(e: &[ChatEvent]) -> Vec<String> {
    e.iter()
        .filter_map(|e| match e {
            ChatEvent::ToolFinished { output, .. } => Some(output.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn commands_are_not_offered_unless_settings_and_the_agent_both_allow_them() {
    let (srv, h, id, _ws) = asking_agent(vec![chat_route(reply("Hi."))]).await;
    // The agent says Ask, but the Settings switch was never turned on.
    h.cmd(ChatCommand::SetCapability(
        id,
        "shell.run".into(),
        crate::session::Permission::Ask,
    ));
    h.send(id, "hello").await;
    h.wait_run_ended(1).await;
    let names: Vec<_> = chat_bodies(&srv)[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap().to_string())
        .collect();
    assert!(!names.contains(&"run_command".to_string()), "{names:?}");
}

#[tokio::test]
async fn a_command_waits_shows_exactly_what_will_run_and_runs_once_allowed() {
    let (_srv, h, id, ws) = commanding_agent(vec![
        chat_route(calls(
            "run_command",
            serde_json::json!({"command": "echo -roost-says-hi"}),
        )),
        chat_route(reply("Done.")),
    ])
    .await;
    h.send(id, "say hi").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    let p = run_preview_of(&h.events()).expect("a command shows what it will run");
    assert_eq!(p.command, "echo -roost-says-hi");
    assert_eq!(p.folder, ws.path());
    assert_eq!(p.prefix.as_deref(), Some("echo"));
    // The card states what will really be kept: the setting's 20 KB, cut to
    // the share of this model's 8192-token window one result may take.
    assert_eq!((p.timeout_secs, p.output_kb), (120, 4));
    assert!(
        tool_outputs(&h.events()).is_empty(),
        "nothing ran before the answer"
    );
    allow_next(&h, id);
    h.wait_run_ended(1).await;
    let out = tool_outputs(&h.events());
    assert!(
        out.iter()
            .any(|o| o.contains("-roost-says-hi") && o.contains("exit code: 0")),
        "{out:?}"
    );
}

#[tokio::test]
async fn a_denied_command_does_not_run() {
    let (_srv, h, id, ws) = commanding_agent(vec![
        chat_route(calls(
            "run_command",
            serde_json::json!({"command": "echo nope > denied.txt"}),
        )),
        chat_route(reply("Understood.")),
    ])
    .await;
    h.send(id, "write a file by shell").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Deny,
    });
    h.wait_run_ended(1).await;
    assert!(!ws.path().join("denied.txt").exists());
}

#[tokio::test]
async fn allowing_a_command_prefix_covers_that_prefix_only() {
    let (_srv, h, id, _ws) = commanding_agent(vec![
        chat_route(calls(
            "run_command",
            serde_json::json!({"command": "echo -one"}),
        )),
        chat_route(calls(
            "run_command",
            serde_json::json!({"command": "echo -two"}),
        )),
        // Chained onto an allowed prefix: must ask again.
        chat_route(calls(
            "run_command",
            serde_json::json!({"command": "echo -three && echo -four"}),
        )),
        chat_route(reply("Done.")),
    ])
    .await;
    h.send(id, "echo things").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::AllowForSession,
    });
    // "echo two" runs without a question; the chained one asks.
    h.wait("asked about the chained command", |e| {
        count_approvals(e) == 2
    })
    .await;
    let out = tool_outputs(&h.events());
    assert!(
        out.iter().any(|o| o.contains("-one")) && out.iter().any(|o| o.contains("-two")),
        "{out:?}"
    );
    assert!(
        !out.iter().any(|o| o.contains("-four")),
        "the chained command waits"
    );
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Deny,
    });
    h.wait_run_ended(1).await;
    // The capability itself never became automatic.
    let s = h.try_read(id).unwrap();
    assert_eq!(
        s.effective_tools.capabilities.get("shell.run"),
        Some(&crate::session::Permission::Ask)
    );
}

#[tokio::test]
async fn a_command_that_cannot_be_covered_offers_no_allowance_and_asks_every_time() {
    let (_srv, h, id, _ws) = commanding_agent(vec![
        chat_route(calls(
            "run_command",
            serde_json::json!({"command": "rm -rf build"}),
        )),
        chat_route(calls(
            "run_command",
            serde_json::json!({"command": "rm -rf build"}),
        )),
        chat_route(reply("Done.")),
    ])
    .await;
    h.send(id, "clean").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    assert_eq!(run_preview_of(&h.events()).unwrap().prefix, None);
    // Even if "allow for this session" arrives anyway, it runs this one only.
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::AllowForSession,
    });
    h.wait("asked again", |e| count_approvals(e) == 2).await;
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Deny,
    });
    h.wait_run_ended(1).await;
}

#[cfg(unix)]
#[tokio::test]
async fn stopping_the_agent_ends_a_running_command() {
    let (_srv, h, id, ws) = commanding_agent(vec![chat_route(calls(
        "run_command",
        serde_json::json!({"command": "sleep 3; echo late > late.txt"}),
    ))])
    .await;
    h.send(id, "run it").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    allow_next(&h, id);
    h.wait("started", |e| {
        e.iter().any(|e| matches!(e, ChatEvent::ToolStarted { .. }))
    })
    .await;
    h.cmd(ChatCommand::Stop(id));
    h.wait_run_ended(1).await;
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    assert!(
        !ws.path().join("late.txt").exists(),
        "the command was killed, not left running"
    );
}

#[tokio::test]
async fn a_denied_write_changes_nothing() {
    let (_srv, h, id, ws) = asking_agent(vec![
        chat_route(calls(
            "write_file",
            serde_json::json!({"path": "a.txt", "content": "overwritten\n"}),
        )),
        chat_route(reply("Understood.")),
    ])
    .await;
    let before = std::fs::read_to_string(ws.path().join("a.txt")).unwrap();
    h.send(id, "overwrite it").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    let p = preview_of(&h.events()).unwrap();
    assert!(!p.created, "an existing file is changed, not created");
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Deny,
    });
    h.wait_run_ended(1).await;
    assert_eq!(
        std::fs::read_to_string(ws.path().join("a.txt")).unwrap(),
        before
    );
}

#[tokio::test]
async fn allowing_edits_to_a_file_covers_that_file_only_and_does_not_make_writing_automatic() {
    let (_srv, h, id, ws) = asking_agent(vec![
        chat_route(calls(
            "write_file",
            serde_json::json!({"path": "x.txt", "content": "1\n"}),
        )),
        chat_route(calls(
            "write_file",
            serde_json::json!({"path": "x.txt", "content": "2\n"}),
        )),
        chat_route(calls(
            "write_file",
            serde_json::json!({"path": "y.txt", "content": "other\n"}),
        )),
        chat_route(reply("Done.")),
    ])
    .await;
    h.send(id, "write x twice and y once").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::AllowForSession,
    });
    // x.txt is written again without asking; y.txt asks.
    h.wait("asked about y", |e| count_approvals(e) == 2).await;
    assert_eq!(
        std::fs::read_to_string(ws.path().join("x.txt")).unwrap(),
        "2\n"
    );
    assert!(!ws.path().join("y.txt").exists());
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Deny,
    });
    h.wait_run_ended(1).await;
    assert!(!ws.path().join("y.txt").exists());
    let s = h.try_read(id).unwrap();
    assert_ne!(
        s.effective_tools.capabilities.get("files.write"),
        Some(&crate::session::Permission::Auto)
    );
}

#[tokio::test]
async fn stopping_while_waiting_for_an_answer_ends_the_run_and_cancels_the_call() {
    let (_srv, h, id, _ws) = asking_agent(vec![chat_route(calls(
        "read_file",
        serde_json::json!({"path": "a.txt"}),
    ))])
    .await;
    h.send(id, "read it").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    h.cmd(ChatCommand::Stop(id));
    h.wait_run_ended(1).await;
    assert_eq!(
        run_ends(&h.events())[0].0,
        super::types::RunOutcome::Cancelled
    );
    h.wait("saved", |_| {
        h.try_read(id).is_some_and(|s| {
            s.messages.iter().any(|m| {
                matches!(
                    &m.content,
                    MessageContent::Tool {
                        status: crate::session::ToolStatus::Cancelled,
                        ..
                    }
                )
            })
        })
    })
    .await;
}

#[tokio::test]
async fn an_answer_for_another_call_is_ignored() {
    let (_srv, h, id, _ws) = asking_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("ok")),
    ])
    .await;
    h.send(id, "read it").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id: "not-the-call".into(),
        decision: ApprovalDecision::Allow,
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(started_tools(&h.events()), 0);
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Allow,
    });
    h.wait_run_ended(1).await;
}

#[tokio::test]
async fn reopening_a_chat_that_is_waiting_asks_the_question_again() {
    let (_srv, h, id, _ws) = asking_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("ok")),
    ])
    .await;
    h.send(id, "read it").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    h.cmd(ChatCommand::Open(id));
    h.wait("asked again", |e| count_approvals(e) >= 2).await;
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Allow,
    });
    h.wait_run_ended(1).await;
}

/// An agent that may read and write without asking, with the given limits.
async fn free_agent(
    routes: Vec<Route>,
    limits: crate::agent::tools::Limits,
) -> (TcpScript, H, SessionId, TempDir) {
    let ws = workspace();
    let srv = fake_agent(routes).await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.cmd(ChatCommand::SetLimits(Box::new(limits)));
    for key in ["files.read", "files.write"] {
        h.cmd(ChatCommand::SetCapability(
            id,
            key.into(),
            crate::session::Permission::Auto,
        ));
    }
    h.wait("capabilities set", |_| {
        h.try_read(id).is_some_and(|s| {
            s.effective_tools.capabilities.get("files.write")
                == Some(&crate::session::Permission::Auto)
        })
    })
    .await;
    (srv, h, id, ws)
}

#[tokio::test]
async fn a_smaller_write_limit_from_settings_refuses_a_bigger_write() {
    let limits = crate::agent::tools::Limits {
        write_bytes: 1024,
        ..Default::default()
    };
    let (_srv, h, id, ws) = free_agent(
        vec![
            chat_route(calls(
                "write_file",
                serde_json::json!({"path": "big.txt", "content": "x".repeat(2000)}),
            )),
            chat_route(reply("Too big.")),
        ],
        limits,
    )
    .await;
    h.send(id, "write it").await;
    h.wait_run_ended(1).await;
    assert!(!ws.path().join("big.txt").exists());
    let out = tool_outputs(&h.events()).join("\n");
    assert!(out.contains("at most 1024"), "{out}");
}

#[tokio::test]
async fn a_larger_write_limit_from_settings_allows_what_the_default_refuses() {
    let limits = crate::agent::tools::Limits {
        write_bytes: 400 * 1024,
        ..Default::default()
    };
    let (_srv, h, id, ws) = free_agent(
        vec![
            chat_route(calls(
                "write_file",
                serde_json::json!({"path": "big.txt", "content": "x".repeat(300 * 1024)}),
            )),
            chat_route(reply("Written.")),
        ],
        limits,
    )
    .await;
    h.send(id, "write it").await;
    h.wait_run_ended(1).await;
    assert_eq!(
        std::fs::metadata(ws.path().join("big.txt")).unwrap().len(),
        300 * 1024
    );
}

#[tokio::test]
async fn the_list_limit_from_settings_cuts_a_listing_and_says_so() {
    let limits = crate::agent::tools::Limits {
        list_entries: 1,
        ..Default::default()
    };
    let (_srv, h, id, ws) = free_agent(
        vec![
            chat_route(calls("list_dir", serde_json::json!({"path": "."}))),
            chat_route(reply("Listed.")),
        ],
        limits,
    )
    .await;
    std::fs::write(ws.path().join("one.txt"), "1").unwrap();
    std::fs::write(ws.path().join("two.txt"), "2").unwrap();
    h.send(id, "list").await;
    h.wait_run_ended(1).await;
    let out = tool_outputs(&h.events()).join("\n");
    assert!(out.contains("[showing entries 1-1 of "), "{out}");
}

#[tokio::test]
async fn the_undo_limit_from_settings_refuses_a_change_it_could_not_take_back() {
    let limits = crate::agent::tools::Limits {
        undo_bytes: 1024 * 1024,
        ..Default::default()
    };
    let edit = |n: u32| {
        chat_route(calls(
            "edit_file",
            serde_json::json!({"path": "big.txt", "old_text": format!("marker{n}"), "new_text": format!("marker{}", n + 1)}),
        ))
    };
    let (_srv, h, id, ws) =
        free_agent(vec![edit(0), edit(1), chat_route(reply("Done."))], limits).await;
    // Each edit keeps the 600 KB original, so the second would pass 1 MiB.
    let body = format!("marker0 {}", "y".repeat(600_000));
    std::fs::write(ws.path().join("big.txt"), body).unwrap();
    h.send(id, "edit twice").await;
    h.wait_run_ended(1).await;
    let out = tool_outputs(&h.events());
    assert!(out[0].starts_with("Changed"), "{out:?}");
    assert!(out[1].contains("cannot keep another copy"), "{out:?}");
}

#[tokio::test]
async fn a_small_context_window_shrinks_what_one_tool_call_may_return() {
    // The test model gets an 8192-token window, so one result may carry
    // about 4 KB even though the setting allows 24 KB.
    let (_srv, h, id, ws) = free_agent(
        vec![
            chat_route(calls(
                "read_file",
                serde_json::json!({"path": "log.txt", "limit": 1000}),
            )),
            chat_route(reply("Read.")),
        ],
        Default::default(),
    )
    .await;
    let lines: String = (1..=400).map(|i| format!("line number {i}\n")).collect();
    std::fs::write(ws.path().join("log.txt"), lines).unwrap();
    h.send(id, "read the log").await;
    h.wait_run_ended(1).await;
    let out = tool_outputs(&h.events()).join("\n");
    assert!(out.len() < 4_400, "{} bytes", out.len());
    assert!(
        out.contains("[truncated; call read_file again with offset="),
        "{out}"
    );
}

#[tokio::test]
async fn a_deleted_chat_comes_back_with_undelete_until_it_is_purged() {
    let srv = fake(vec![]).await;
    let h = start(&srv).await;
    let id = h.create(SessionLifetime::Persisted).await;
    h.wait("listed", |e| last_rows(e).iter().any(|r| r.id == id))
        .await;
    h.cmd(ChatCommand::Delete(id));
    h.wait("gone", |e| !last_rows(e).iter().any(|r| r.id == id))
        .await;
    assert!(!h.file(id).exists());
    h.cmd(ChatCommand::Undelete(id));
    h.wait("back", |e| last_rows(e).iter().any(|r| r.id == id))
        .await;
    assert!(h.file(id).exists());
    // Purged for good: the undo no longer works.
    h.cmd(ChatCommand::Delete(id));
    h.wait("gone again", |e| !last_rows(e).iter().any(|r| r.id == id))
        .await;
    h.cmd(ChatCommand::PurgeDeleted(id));
    h.cmd(ChatCommand::Undelete(id));
    h.handle.shutdown().await;
    assert!(!h.file(id).exists());
}

#[tokio::test]
async fn every_model_request_of_an_agent_run_announces_one_turn_start() {
    let ws = workspace();
    let srv = fake_agent(vec![
        chat_route(calls("list_dir", serde_json::json!({}))),
        chat_route(calls("list_dir", serde_json::json!({}))),
        chat_route(calls("list_dir", serde_json::json!({}))),
        chat_route(reply("done")),
    ])
    .await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.send(id, "list a few times").await;
    h.wait_run_ended(1).await;
    let starts = h
        .events()
        .iter()
        .filter(|e| matches!(e, ChatEvent::TurnStarted { session, .. } if *session == id))
        .count();
    assert_eq!(
        starts,
        chat_bodies(&srv).len(),
        "one turn start per request"
    );
    assert_eq!(starts, 4);
}

// ------------------------------------------------- how an agent run ended

async fn saved_end(h: &H, id: SessionId) -> Option<crate::session::RunEndNote> {
    // The note is written with the session, a moment after the run ends.
    let mut last = None;
    for _ in 0..40 {
        last = h.try_read(id).and_then(|s| s.last_run_end);
        if last.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    last
}

#[tokio::test]
async fn an_agent_that_runs_out_of_steps_saves_why_and_a_later_finish_clears_it() {
    use crate::session::RunEnd;
    let ws = workspace();
    let srv = fake_agent(vec![
        chat_route(calls("list_dir", serde_json::json!({}))),
        chat_route(calls("list_dir", serde_json::json!({}))),
        chat_route(reply("all done")),
    ])
    .await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.cmd(ChatCommand::SetStepBudget(id, 2));
    h.send(id, "list forever").await;
    h.wait_run_ended(1).await;
    let note = saved_end(&h, id).await.expect("a saved ending");
    assert_eq!(note.end, RunEnd::StepLimit { steps: 2 });
    assert_eq!(
        note.after,
        h.read(id).messages.len(),
        "at the end of the chat"
    );
    // Continue with a normal finish: the note goes.
    h.send(id, "Continue").await;
    h.wait_run_ended(2).await;
    h.wait("cleared", |_| {
        h.try_read(id).is_some_and(|s| s.last_run_end.is_none())
    })
    .await;
}

#[tokio::test]
async fn stopping_an_agent_saves_that_you_stopped_it() {
    use crate::session::RunEnd;
    let ws = workspace();
    let (_keep, hold) = oneshot::channel::<()>();
    let srv = fake_agent(vec![chat_route(vec![
        Step::Write(line("thinking")),
        Step::WaitFor(hold),
    ])])
    .await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait("streaming", |_| h.deltas(id) == "thinking").await;
    h.cmd(ChatCommand::Stop(id));
    h.wait_run_ended(1).await;
    assert_eq!(saved_end(&h, id).await.unwrap().end, RunEnd::Stopped);
}

#[tokio::test]
async fn a_failed_agent_run_saves_the_reason_and_a_chat_saves_nothing() {
    use crate::session::RunEnd;
    let ws = workspace();
    let srv = fake_agent(vec![chat_route(vec![
        Step::Write(line("x")),
        Step::Pause(Duration::from_secs(5)),
    ])])
    .await;
    let c = client(&srv).with_stall_timeout(Duration::from_millis(150));
    let h = start_in(TempDir::new().unwrap(), c, |_| {}).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.send(id, "go").await;
    h.wait_run_ended(1).await;
    match saved_end(&h, id).await.expect("a saved ending").end {
        RunEnd::Failed { message } => assert!(message.contains("stopped responding"), "{message}"),
        other => panic!("{other:?}"),
    }
    // The same stall in a plain chat leaves no note (the reply itself says so).
    let chat = h.create(SessionLifetime::Persisted).await;
    h.send(chat, "go").await;
    h.wait_ended(1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.read(chat).last_run_end, None);
}

#[tokio::test]
async fn a_reply_that_is_only_a_tool_call_written_as_text_is_reported_not_called_finished() {
    use crate::session::RunEnd;
    let ws = workspace();
    let srv = fake_agent(vec![chat_route(reply(
        r#"{"name": "list_dir", "arguments": {}}"#,
    ))])
    .await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.send(id, "what is in here?").await;
    h.wait_run_ended(1).await;
    let (outcome, message) = run_ends(&h.events())[0].clone();
    assert_eq!(outcome, super::types::RunOutcome::TextToolCall);
    assert_eq!(message.as_deref(), Some("list_dir"));
    assert_eq!(
        chat_bodies(&srv).len(),
        1,
        "nothing was run, no second request"
    );
    assert_eq!(
        saved_end(&h, id).await.unwrap().end,
        RunEnd::TextToolCall {
            tool: "list_dir".into()
        }
    );
}

#[tokio::test]
async fn an_ordinary_answer_that_mentions_json_still_finishes_normally() {
    let ws = workspace();
    let srv = fake_agent(vec![chat_route(reply(
        r#"The call would look like {"name": "list_dir", "arguments": {}}, but I did not run it."#,
    ))])
    .await;
    let h = start(&srv).await;
    let id = h.create_agent(&ws, SessionLifetime::Persisted).await;
    h.send(id, "show me an example call").await;
    h.wait_run_ended(1).await;
    assert_eq!(
        run_ends(&h.events())[0].0,
        super::types::RunOutcome::Completed
    );
}

async fn web_agent(routes: Vec<Route>) -> (TcpScript, H, SessionId, TempDir) {
    let (srv, h, id, ws) = asking_agent(routes).await;
    h.cmd(ChatCommand::SetCapability(
        id,
        "web.fetch".into(),
        crate::session::Permission::Ask,
    ));
    h.wait("web set to ask", |_| {
        h.try_read(id).is_some_and(|s| {
            s.effective_tools.capabilities.get("web.fetch")
                == Some(&crate::session::Permission::Ask)
        })
    })
    .await;
    (srv, h, id, ws)
}

#[tokio::test]
async fn the_web_tool_is_not_offered_until_the_agent_turns_the_web_on() {
    let (srv, h, id, _ws) = asking_agent(vec![chat_route(reply("Hi."))]).await;
    h.send(id, "hello").await;
    h.wait_run_ended(1).await;
    let names: Vec<_> = chat_bodies(&srv)[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap().to_string())
        .collect();
    assert!(!names.contains(&"fetch_url".to_string()), "{names:?}");
}

#[tokio::test]
async fn a_fetch_waits_for_the_user_and_a_declined_one_fetches_nothing() {
    let (_srv, h, id, _ws) = web_agent(vec![
        chat_route(calls(
            "fetch_url",
            serde_json::json!({"url": "https://example.com/page"}),
        )),
        chat_route(reply("Done.")),
    ])
    .await;
    h.send(id, "look it up").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    let (call_id, name) = approval_for(&h.events()).unwrap();
    assert_eq!(name, "fetch_url");
    assert!(
        tool_outputs(&h.events()).is_empty(),
        "nothing was fetched before the answer"
    );
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Deny,
    });
    h.wait_run_ended(1).await;
    let out = tool_outputs(&h.events());
    assert!(
        out.iter().all(|o| !o.contains("Text from the web")),
        "{out:?}"
    );
}

#[tokio::test]
async fn a_local_address_is_refused_with_the_reason_even_when_the_user_says_yes() {
    let (_srv, h, id, _ws) = web_agent(vec![
        chat_route(calls(
            "fetch_url",
            serde_json::json!({"url": "https://127.0.0.1:9/secret"}),
        )),
        chat_route(reply("Done.")),
    ])
    .await;
    h.send(id, "look at it").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    allow_next(&h, id);
    h.wait_run_ended(1).await;
    let out = tool_outputs(&h.events());
    assert!(
        out.iter().any(|o| o.contains("agents may not fetch")),
        "{out:?}"
    );
}

#[tokio::test]
async fn allowing_a_host_covers_that_host_for_the_chat_and_never_the_whole_web() {
    let (_srv, h, id, _ws) = web_agent(vec![
        chat_route(calls(
            "fetch_url",
            serde_json::json!({"url": "https://localhost:9/one"}),
        )),
        // The same host: no second question.
        chat_route(calls(
            "fetch_url",
            serde_json::json!({"url": "https://LOCALHOST:9/two"}),
        )),
        // Another host asks again.
        chat_route(calls(
            "fetch_url",
            serde_json::json!({"url": "https://127.0.0.1:9/three"}),
        )),
        chat_route(reply("Done.")),
    ])
    .await;
    h.send(id, "fetch things").await;
    h.wait("asked", |e| approval_for(e).is_some()).await;
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::AllowForSession,
    });
    h.wait("asked about the other host", |e| count_approvals(e) == 2)
        .await;
    let (call_id, _) = approval_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ResolveApproval {
        session: id,
        call_id,
        decision: ApprovalDecision::Deny,
    });
    h.wait_run_ended(1).await;
    assert_eq!(
        count_approvals(&h.events()),
        2,
        "the second call needed no question"
    );
    // The capability itself never became automatic.
    let s = h.try_read(id).unwrap();
    assert_eq!(
        s.effective_tools.capabilities.get("web.fetch"),
        Some(&crate::session::Permission::Ask)
    );
}

// ---- an agent without a folder

impl H {
    async fn create_unattached_agent(&self) -> SessionId {
        let before = self.opened().len();
        self.cmd(ChatCommand::CreateAgent {
            lifetime: SessionLifetime::Persisted,
            model: "llama3:8b".into(),
            workspace: None,
        });
        self.wait("agent opened", |e| opened(e).len() > before)
            .await;
        *self.opened().last().unwrap()
    }
}

fn workspace_needed_for(e: &[ChatEvent]) -> Option<(String, String)> {
    e.iter().rev().find_map(|e| match e {
        ChatEvent::WorkspaceNeeded { call_id, name, .. } => Some((call_id.clone(), name.clone())),
        _ => None,
    })
}

fn count_workspace_needed(e: &[ChatEvent]) -> usize {
    e.iter()
        .filter(|e| matches!(e, ChatEvent::WorkspaceNeeded { .. }))
        .count()
}

/// The saved tool message of `name`, once there is one: (status, output).
fn saved_tool(h: &H, id: SessionId, name: &str) -> Option<(crate::session::ToolStatus, String)> {
    h.try_read(id)?
        .messages
        .iter()
        .find_map(|m| match &m.content {
            MessageContent::Tool {
                name: n,
                status,
                output,
                ..
            } if n == name => Some((*status, output.clone().unwrap_or_default())),
            _ => None,
        })
}

/// An agent with no folder whose first model step asks to read `a.txt`.
async fn unattached_agent(routes: Vec<Route>) -> (TcpScript, H, SessionId) {
    let srv = fake_agent(routes).await;
    let h = start(&srv).await;
    let id = h.create_unattached_agent().await;
    (srv, h, id)
}

#[tokio::test]
async fn an_agent_can_be_created_without_a_folder() {
    let (_srv, h, id) = unattached_agent(vec![]).await;
    h.wait("saved", |_| h.try_read(id).is_some()).await;
    let s = h.try_read(id).unwrap();
    assert_eq!(s.mode, crate::session::SessionMode::Agent);
    assert_eq!(s.workspace, None);
}

#[tokio::test]
async fn a_file_call_without_a_folder_waits_and_runs_once_one_is_chosen() {
    let ws = workspace();
    let (_srv, h, id) = unattached_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("It says hello.")),
    ])
    .await;
    h.send(id, "what is in a.txt?").await;
    h.wait("folder needed", |e| workspace_needed_for(e).is_some())
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        started_tools(&h.events()),
        0,
        "nothing runs without a folder"
    );
    assert!(run_ends(&h.events()).is_empty(), "the run is waiting");
    let (call_id, name) = workspace_needed_for(&h.events()).unwrap();
    assert_eq!(name, "read_file");
    h.cmd(ChatCommand::ProvideWorkspace {
        session: id,
        call_id,
        path: Some(ws.path().to_path_buf()),
    });
    h.wait_run_ended(1).await;
    assert_eq!(
        run_ends(&h.events()),
        vec![(super::types::RunOutcome::Completed, None)]
    );
    assert_eq!(started_tools(&h.events()), 1);
    h.wait("saved", |_| {
        h.try_read(id)
            .is_some_and(|s| s.workspace.as_deref() == Some(ws.path()))
    })
    .await;
    let (status, output) = saved_tool(&h, id, "read_file").unwrap();
    assert_eq!(status, crate::session::ToolStatus::Complete);
    assert!(output.contains("hello"), "{output}");
}

#[tokio::test]
async fn skipping_the_folder_fails_the_call_and_the_run_goes_on() {
    let (_srv, h, id) = unattached_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("I cannot read files here.")),
    ])
    .await;
    h.send(id, "what is in a.txt?").await;
    h.wait("folder needed", |e| workspace_needed_for(e).is_some())
        .await;
    let (call_id, _) = workspace_needed_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ProvideWorkspace {
        session: id,
        call_id,
        path: None,
    });
    h.wait_run_ended(1).await;
    assert_eq!(
        run_ends(&h.events()),
        vec![(super::types::RunOutcome::Completed, None)]
    );
    assert_eq!(started_tools(&h.events()), 0);
    h.wait("saved", |_| saved_tool(&h, id, "read_file").is_some())
        .await;
    let (status, output) = saved_tool(&h, id, "read_file").unwrap();
    assert_eq!(status, crate::session::ToolStatus::Failed);
    assert!(output.contains("No working directory"), "{output}");
    assert_eq!(h.try_read(id).unwrap().workspace, None);
}

#[tokio::test]
async fn a_folder_that_cannot_be_used_leaves_the_call_waiting() {
    let ws = workspace();
    let (_srv, h, id) = unattached_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("It says hello.")),
    ])
    .await;
    h.send(id, "what is in a.txt?").await;
    h.wait("folder needed", |e| workspace_needed_for(e).is_some())
        .await;
    let (call_id, _) = workspace_needed_for(&h.events()).unwrap();
    let nowhere = TempDir::new().unwrap().path().join("missing");
    h.cmd(ChatCommand::ProvideWorkspace {
        session: id,
        call_id: call_id.clone(),
        path: Some(nowhere),
    });
    h.wait("refused", |e| {
        e.iter()
            .any(|e| matches!(e, ChatEvent::SessionError { session: Some(s), .. } if *s == id))
    })
    .await;
    assert!(run_ends(&h.events()).is_empty(), "still waiting");
    h.cmd(ChatCommand::ProvideWorkspace {
        session: id,
        call_id,
        path: Some(ws.path().to_path_buf()),
    });
    h.wait_run_ended(1).await;
    assert_eq!(started_tools(&h.events()), 1);
}

#[tokio::test]
async fn a_switched_off_file_tool_fails_without_asking_for_a_folder() {
    let (_srv, h, id) = unattached_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("Reading is off.")),
    ])
    .await;
    h.cmd(ChatCommand::SetCapability(
        id,
        "files.read".into(),
        crate::session::Permission::Off,
    ));
    h.wait("capability set", |_| {
        h.try_read(id).is_some_and(|s| {
            s.effective_tools.capabilities.get("files.read")
                == Some(&crate::session::Permission::Off)
        })
    })
    .await;
    h.send(id, "what is in a.txt?").await;
    h.wait_run_ended(1).await;
    assert_eq!(count_workspace_needed(&h.events()), 0);
}

#[tokio::test]
async fn reopening_an_agent_that_waits_for_a_folder_asks_again() {
    let ws = workspace();
    let (_srv, h, id) = unattached_agent(vec![
        chat_route(calls("read_file", serde_json::json!({"path": "a.txt"}))),
        chat_route(reply("It says hello.")),
    ])
    .await;
    h.send(id, "what is in a.txt?").await;
    h.wait("folder needed", |e| workspace_needed_for(e).is_some())
        .await;
    h.cmd(ChatCommand::Close(id));
    h.cmd(ChatCommand::Open(id));
    h.wait("asked again", |e| count_workspace_needed(e) >= 2)
        .await;
    let (call_id, _) = workspace_needed_for(&h.events()).unwrap();
    h.cmd(ChatCommand::ProvideWorkspace {
        session: id,
        call_id,
        path: Some(ws.path().to_path_buf()),
    });
    h.wait_run_ended(1).await;
    assert_eq!(started_tools(&h.events()), 1);
}

#[tokio::test]
async fn stopping_an_agent_that_waits_for_a_folder_ends_the_run() {
    let (_srv, h, id) = unattached_agent(vec![chat_route(calls(
        "read_file",
        serde_json::json!({"path": "a.txt"}),
    ))])
    .await;
    h.send(id, "what is in a.txt?").await;
    h.wait("folder needed", |e| workspace_needed_for(e).is_some())
        .await;
    h.cmd(ChatCommand::Stop(id));
    h.wait_run_ended(1).await;
    assert_eq!(
        run_ends(&h.events()),
        vec![(super::types::RunOutcome::Cancelled, None)]
    );
}

// ---- an agent without a model

#[tokio::test]
async fn an_agent_without_a_model_cannot_send_until_one_is_chosen() {
    let srv = fake_agent(vec![chat_route(reply("Hello."))]).await;
    let h = start(&srv).await;
    h.cmd(ChatCommand::CreateAgent {
        lifetime: SessionLifetime::Persisted,
        model: String::new(),
        workspace: None,
    });
    h.wait("agent opened", |e| !opened(e).is_empty()).await;
    let id = *h.opened().last().unwrap();
    h.send(id, "hi").await;
    h.wait("rejected", |e| {
        e.iter().any(|e| {
            matches!(e, ChatEvent::SendRejected { reason: SendRejection::NoModel, session } if *session == id)
        })
    })
    .await;
    assert!(run_ends(&h.events()).is_empty());
    h.cmd(ChatCommand::SetModel(id, "llama3:8b".into()));
    h.wait("model saved", |_| {
        h.try_read(id).is_some_and(|s| s.model == "llama3:8b")
    })
    .await;
    h.send(id, "hi").await;
    h.wait_run_ended(1).await;
    assert_eq!(
        run_ends(&h.events()),
        vec![(super::types::RunOutcome::Completed, None)]
    );
}
