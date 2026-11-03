//! The session manager: one task that owns every open session, the incognito
//! map and the rule of at most one reply per session. The GUI sends intent
//! commands and receives ordered events; all disk work goes through the store
//! writer and all HTTP through run tasks.

use super::agent_loop::{AgentRun, ToolRun};
use super::context::{build_request, window};
use super::run::{answer_once, drive, RunMsg};
use super::types::{
    BranchState, ChatCommand, ChatEvent, EventSink, RunOutcome, SendRejection, TurnOutcome,
};
use crate::error::Error;
use crate::ollama::{ChatRequestBody, OllamaClient, StreamEvent};
use crate::session::{
    AssistantStatus, BtwEntry, BtwId, Message, MessageContent, MessageId, RunId, Session,
    SessionError, SessionId, SessionLifetime, SessionMode, TurnId, UserDelivery,
};
use crate::store::{live_row, IndexRow, LoadError, StoreWarning, Writer};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

pub const CHAT_SYSTEM_PROMPT: &str = "You are Roost, a helpful assistant running locally. Be concise, be honest about uncertainty, and do not claim to have used tools unless a tool result appears in this conversation.";
const FLUSH_INTERVAL: Duration = Duration::from_secs(2);
const COALESCE: Duration = Duration::from_millis(30);
const SHUTDOWN_LIMIT: Duration = Duration::from_secs(3);

pub struct ChatConfig {
    pub ollama: OllamaClient,
    /// How often a streaming reply is written to disk.
    pub flush_interval: Duration,
    /// Content deltas are batched for this long before an event is sent.
    pub coalesce: Duration,
}

impl ChatConfig {
    pub fn new(ollama: OllamaClient) -> ChatConfig {
        ChatConfig {
            ollama,
            flush_interval: FLUSH_INTERVAL,
            coalesce: COALESCE,
        }
    }
}

enum Cmd {
    User(ChatCommand),
    SetClient(Box<OllamaClient>),
    Shutdown(oneshot::Sender<()>),
}

#[derive(Clone)]
pub struct ChatHandle {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl ChatHandle {
    pub fn send(&self, command: ChatCommand) {
        let _ = self.tx.send(Cmd::User(command));
    }

    /// Uses a rebuilt client (settings changed) for future requests.
    pub fn set_ollama(&self, client: OllamaClient) {
        let _ = self.tx.send(Cmd::SetClient(Box::new(client)));
    }

    /// Stops every run, writes what is partial and destroys incognito state.
    /// Gives up after three seconds.
    pub async fn shutdown(&self) {
        let (ack, done) = oneshot::channel();
        if self.tx.send(Cmd::Shutdown(ack)).is_ok() {
            let _ = tokio::time::timeout(SHUTDOWN_LIMIT, done).await;
        }
    }
}

pub struct ChatManager;

impl ChatManager {
    pub fn spawn(
        rt: &tokio::runtime::Handle,
        writer: Writer,
        cfg: ChatConfig,
        sink: EventSink,
    ) -> ChatHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let (run_tx, run_rx) = mpsc::unbounded_channel();
        let manager = Manager {
            writer,
            client: cfg.ollama,
            flush_interval: cfg.flush_interval,
            coalesce: cfg.coalesce,
            sink,
            sessions: HashMap::new(),
            ctx_cache: HashMap::new(),
            run_tx,
            next_token: 0,
            writes_allowed: true,
            limits: Default::default(),
            commands_allowed: false,
        };
        rt.spawn(manager.run(rx, run_rx));
        ChatHandle { tx }
    }
}

/// What `/api/show` told us about a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ModelInfo {
    pub context: Option<u64>,
    pub tools: bool,
}

#[derive(Clone)]
pub(super) enum Intent {
    Send(String),
    Retry,
    Regenerate,
    Edit(MessageId, String),
}

pub(super) struct Run {
    pub(super) turn: TurnId,
    pub(super) assistant: MessageId,
    pub(super) stop: watch::Sender<bool>,
    pub(super) join: JoinHandle<()>,
    pub(super) content: String,
    pub(super) thinking: String,
    pub(super) pending_since: Option<Instant>,
}

pub(super) enum Slot {
    Idle,
    Preparing {
        token: u64,
        intent: Intent,
    },
    /// A model reply is streaming.
    Running(Box<Run>),
    /// An agent's tool call is running.
    Tools(Box<ToolRun>),
}

pub(super) struct Live {
    pub(super) s: Session,
    pub(super) slot: Slot,
    pub(super) close_when_idle: bool,
    pub(super) last_flush: Instant,
    /// Text arrived since the last write to disk.
    pub(super) dirty: bool,
    /// The loop state while an agent run is active.
    pub(super) agent: Option<AgentRun>,
    /// Files whose edits the user allowed for as long as this chat is open.
    pub(super) allowed_files: std::collections::HashSet<String>,
    /// Command prefixes the user allowed for as long as this chat is open.
    pub(super) allowed_prefixes: std::collections::HashSet<String>,
    /// Hosts the user allowed web fetches from, for this chat only.
    pub(super) allowed_hosts: std::collections::HashSet<String>,
}

impl Live {
    pub(super) fn new(s: Session) -> Live {
        Live {
            s,
            slot: Slot::Idle,
            close_when_idle: false,
            last_flush: Instant::now(),
            dirty: false,
            agent: None,
            allowed_files: Default::default(),
            allowed_prefixes: Default::default(),
            allowed_hosts: Default::default(),
        }
    }
}

pub(super) struct Manager {
    pub(super) writer: Writer,
    pub(super) client: OllamaClient,
    pub(super) flush_interval: Duration,
    pub(super) coalesce: Duration,
    pub(super) sink: EventSink,
    pub(super) sessions: HashMap<SessionId, Live>,
    /// Model name to what `/api/show` said.
    pub(super) ctx_cache: HashMap<String, ModelInfo>,
    pub(super) run_tx: mpsc::UnboundedSender<RunMsg>,
    pub(super) next_token: u64,
    /// Settings: may agents write files at all.
    pub(super) writes_allowed: bool,
    /// Settings: may agents run commands at all (off until the user says).
    pub(super) commands_allowed: bool,
    /// Settings: the limits the agent tools run under.
    pub(super) limits: crate::agent::tools::Limits,
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub(super) fn error_text(e: &Error) -> String {
    match e {
        Error::OllamaUnreachable => {
            "Ollama is not reachable. Make sure it is running, then retry.".into()
        }
        Error::ModelNotFound(m) => format!("The model {m} is not installed. Choose another model."),
        Error::Network(m) if m == "ollama stalled" => {
            "Ollama stopped responding. Retry when it is ready.".into()
        }
        other => other.to_string(),
    }
}

fn warning_text(w: &StoreWarning) -> String {
    match w {
        StoreWarning::IndexRebuilt => {
            "The history index was rebuilt from the session files.".into()
        }
        StoreWarning::Quarantined(_) => {
            "A damaged session file was set aside and kept for diagnosis.".into()
        }
        StoreWarning::NewerSchema(_) => {
            "A session was made by a newer Roost and was left untouched.".into()
        }
        StoreWarning::Unreadable(_) => "A session file could not be read right now.".into(),
    }
}

/// The parts of `/api/show` the manager uses.
fn model_info(d: crate::ollama::ModelDetails) -> ModelInfo {
    ModelInfo {
        context: d.context,
        tools: d.capabilities.iter().any(|c| c == "tools"),
    }
}

fn load_error_text(e: &LoadError) -> String {
    e.to_string()
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

impl Manager {
    pub(super) fn emit(&self, ev: ChatEvent) {
        (self.sink)(ev);
    }

    pub(super) async fn emit_rows(&self) {
        let mut rows: Vec<IndexRow> = self.writer.snapshot().await;
        rows.extend(
            self.sessions
                .values()
                .filter(|l| l.s.lifetime == SessionLifetime::Incognito)
                .map(|l| live_row(&l.s)),
        );
        self.emit(ChatEvent::Rows(rows));
    }

    pub(super) fn save(&self, s: &Session) {
        if s.lifetime == SessionLifetime::Persisted {
            self.writer.save(s.clone());
        }
    }

    async fn run(
        mut self,
        mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
        mut run_rx: mpsc::UnboundedReceiver<RunMsg>,
    ) {
        for w in self.writer.take_warnings() {
            self.emit(ChatEvent::StoreWarning(warning_text(&w)));
        }
        self.emit_rows().await;
        loop {
            let deadline = self.next_deadline();
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    Some(c) => if self.on_cmd(c).await { break },
                    None => break,
                },
                Some(msg) = run_rx.recv() => self.on_run_msg(msg).await,
                _ = sleep_until_opt(deadline) => self.on_tick(),
            }
            for e in self.writer.take_errors() {
                self.emit(ChatEvent::SessionError {
                    session: None,
                    message: e,
                });
            }
        }
    }

    /// Returns true when the manager should stop.
    async fn on_cmd(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::SetClient(c) => self.client = *c,
            Cmd::Shutdown(ack) => {
                self.shutdown().await;
                let _ = ack.send(());
                return true;
            }
            Cmd::User(c) => self.on_command(c).await,
        }
        false
    }

    async fn on_command(&mut self, cmd: ChatCommand) {
        match cmd {
            ChatCommand::Create { lifetime, model } => self.create(lifetime, &model).await,
            ChatCommand::Open(id) => self.open(id).await,
            ChatCommand::Close(id) => self.close(id).await,
            ChatCommand::ClearIncognito => self.clear_incognito().await,
            ChatCommand::Rename(id, title) => {
                self.edit(id, move |s| s.rename(&title)).await;
            }
            ChatCommand::SetPinned(id, pinned) => {
                self.edit(id, move |s| {
                    s.set_pinned(pinned);
                    Ok(())
                })
                .await;
            }
            ChatCommand::SetModel(id, model) => self.set_model(id, model).await,
            ChatCommand::Delete(id) => self.delete(id).await,
            ChatCommand::Undelete(id) => {
                if self.writer.restore(id).await {
                    self.emit_rows().await;
                }
            }
            ChatCommand::PurgeDeleted(id) => self.writer.purge(id),
            ChatCommand::Send { session, text } => self.begin(session, Intent::Send(text)).await,
            ChatCommand::Retry(id) => self.begin(id, Intent::Retry).await,
            ChatCommand::Regenerate(id) => self.begin(id, Intent::Regenerate).await,
            ChatCommand::Edit {
                session,
                message,
                text,
            } => self.begin(session, Intent::Edit(message, text)).await,
            ChatCommand::Branch {
                session,
                message,
                model,
            } => self.branch(session, message, model).await,
            ChatCommand::CheckBranch(id) => self.check_branch(id).await,
            ChatCommand::ResolveApproval {
                session,
                call_id,
                decision,
            } => self.resolve_approval(session, call_id, decision).await,
            ChatCommand::AnswerQuestion {
                session,
                call_id,
                answer,
            } => self.answer_question(session, call_id, answer).await,
            ChatCommand::Btw {
                session,
                message,
                question,
            } => self.btw(session, message, question).await,
            ChatCommand::BtwAdd { session, id } => self.btw_add(session, id).await,
            ChatCommand::BtwDelete { session, id } => {
                self.edit(session, move |s| {
                    if s.delete_btw(id) {
                        Ok(())
                    } else {
                        Err(SessionError::UnknownMessage)
                    }
                })
                .await;
                self.emit(ChatEvent::BtwDeleted { session, id });
            }
            ChatCommand::SwitchVersion { session, after, to } => {
                self.switch_version(session, after, to).await
            }
            ChatCommand::Stop(id) => self.stop(id).await,
            ChatCommand::CreateAgent {
                lifetime,
                model,
                workspace,
            } => self.create_agent(lifetime, &model, workspace).await,
            ChatCommand::CheckModel(model) => self.check_model(model),
            ChatCommand::SetWorkspace(id, path) => self.set_workspace(id, path).await,
            ChatCommand::ProvideWorkspace {
                session,
                call_id,
                path,
            } => self.provide_workspace(session, call_id, path).await,
            ChatCommand::SetWritesAllowed(on) => self.writes_allowed = on,
            ChatCommand::SetRunAllowed(on) => self.commands_allowed = on,
            ChatCommand::SetLimits(l) => self.limits = l.clamped(),
            ChatCommand::UndoChange { session, change } => self.undo(session, Some(change)),
            ChatCommand::UndoAll(session) => self.undo(session, None),
            ChatCommand::SetStepBudget(id, steps) => {
                self.edit_idle(id, move |s| {
                    s.step_budget = steps.clamp(1, 100);
                    Ok(())
                })
                .await;
            }
            ChatCommand::SetToolEnabled(id, name, on) => {
                self.edit_idle(id, move |s| {
                    s.set_tool_enabled(&name, on);
                    Ok(())
                })
                .await;
            }
            ChatCommand::SetCapability(id, key, permission) => {
                self.edit_idle(id, move |s| {
                    s.set_capability(&key, permission);
                    Ok(())
                })
                .await;
            }
            ChatCommand::ListSessions => self.emit_rows().await,
            ChatCommand::InstalledChanged => self.ctx_cache.clear(),
        }
    }

    pub(super) fn session_error(&self, id: Option<SessionId>, message: String) {
        self.emit(ChatEvent::SessionError {
            session: id,
            message,
        });
    }

    /// Loads a saved session into memory if it is not there already.
    pub(super) async fn ensure(&mut self, id: SessionId) -> Result<(), LoadError> {
        if self.sessions.contains_key(&id) {
            return Ok(());
        }
        let s = self.writer.load(id).await?;
        self.sessions.insert(id, Live::new(s));
        Ok(())
    }

    async fn create(&mut self, lifetime: SessionLifetime, model: &str) {
        let s = Session::new_chat(model, lifetime, now_ms());
        self.save(&s);
        self.emit(ChatEvent::Opened(Box::new(s.clone())));
        self.sessions.insert(s.id, Live::new(s));
        self.emit_rows().await;
    }

    async fn open(&mut self, id: SessionId) {
        if let Err(e) = self.ensure(id).await {
            self.session_error(Some(id), load_error_text(&e));
            self.emit_rows().await;
            return;
        }
        let mut pending = None;
        let mut snapshot = None;
        let mut waiting = None;
        let lim = self.limits_for(id);
        if let Some(l) = self.sessions.get_mut(&id) {
            l.close_when_idle = false;
            // Text still waiting to be batched is already in the snapshot, so
            // send it first; otherwise the viewer would append it twice.
            if let Slot::Running(r) = &mut l.slot {
                if !r.content.is_empty() || !r.thinking.is_empty() {
                    pending = Some((
                        r.turn,
                        std::mem::take(&mut r.content),
                        std::mem::take(&mut r.thinking),
                    ));
                }
                r.pending_since = None;
            }
            snapshot = Some(l.s.clone());
            // A call still waiting for the user is asked about again, since
            // the window forgot it when it left this chat.
            if let Slot::Tools(t) = &l.slot {
                waiting = t.waiting_call().map(|(call_id, name, arguments)| {
                    let preview = l
                        .agent
                        .as_ref()
                        .and_then(|a| a.preview(&name, &arguments, &lim));
                    let run = l
                        .agent
                        .as_ref()
                        .and_then(|a| a.run_preview(&name, &arguments, &lim));
                    (call_id, name, arguments, preview, run)
                });
            }
        }
        if let Some((turn, content, thinking)) = pending {
            self.emit(ChatEvent::Delta {
                session: id,
                turn,
                content,
                thinking,
            });
        }
        if let Some(s) = snapshot {
            let changes = s.changes.clone();
            self.emit(ChatEvent::Opened(Box::new(s)));
            if !changes.is_empty() {
                self.emit(ChatEvent::Changes {
                    session: id,
                    changes,
                });
            }
        }
        if let Some((call_id, name, arguments, preview, run)) = waiting {
            let folderless = self
                .sessions
                .get(&id)
                .and_then(|l| l.agent.as_ref())
                .is_some_and(|a| !a.has_workspace());
            if folderless && crate::agent::registry::needs_workspace(&name) {
                self.emit(ChatEvent::WorkspaceNeeded {
                    session: id,
                    call_id,
                    name,
                    arguments,
                });
                return;
            }
            if name == crate::agent::ask::TOOL {
                if let Ok(question) = crate::agent::ask::parse(&arguments) {
                    self.emit(ChatEvent::QuestionAsked {
                        session: id,
                        call_id,
                        question,
                    });
                }
                return;
            }
            self.emit(ChatEvent::ApprovalRequested {
                session: id,
                call_id,
                name,
                arguments,
                preview,
                run,
            });
        }
    }

    async fn close(&mut self, id: SessionId) {
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        if live.s.lifetime == SessionLifetime::Incognito {
            if let Some(live) = self.sessions.remove(&id) {
                self.halt(live).await;
            }
            self.emit_rows().await;
        } else if matches!(live.slot, Slot::Idle) {
            self.sessions.remove(&id);
        } else {
            live.close_when_idle = true;
        }
    }

    async fn clear_incognito(&mut self) {
        let ids: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, l)| l.s.lifetime == SessionLifetime::Incognito)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(live) = self.sessions.remove(&id) {
                self.halt(live).await;
            }
        }
        self.emit_rows().await;
    }

    /// Asks a side question about the history through `message`. It runs on
    /// its own, so it works while a reply or an agent run is going.
    async fn btw(&mut self, id: SessionId, message: MessageId, question: String) {
        let question = question.trim().to_string();
        if question.is_empty() {
            self.session_error(Some(id), "Type a question first.".into());
            return;
        }
        if let Err(e) = self.ensure(id).await {
            self.session_error(Some(id), load_error_text(&e));
            return;
        }
        let Some(live) = self.sessions.get(&id) else {
            return;
        };
        let Some(at) = live.s.messages.iter().position(|m| m.id == message) else {
            self.session_error(Some(id), "That message is no longer in this chat.".into());
            return;
        };
        let now = now_ms();
        let mut context = live.s.messages[..=at].to_vec();
        context.push(Message {
            id: MessageId::new(),
            created_at_ms: now,
            content: MessageContent::User {
                text: question.clone(),
                delivery: UserDelivery::Normal,
            },
        });
        let ctx = self.ctx_cache.get(&live.s.model).and_then(|i| i.context);
        let win = window(live.s.model_options.num_ctx, ctx);
        let trimmed = match build_request(CHAT_SYSTEM_PROMPT, &context, win) {
            Ok(t) => t,
            Err(e) => {
                self.session_error(Some(id), e.to_string());
                return;
            }
        };
        let body = ChatRequestBody {
            model: live.s.model.clone(),
            messages: trimmed.messages,
            num_ctx: win,
            temperature: live.s.model_options.temperature,
            seed: live.s.model_options.seed,
            think: live.s.model_options.think,
            tools: None,
        };
        let entry = BtwEntry {
            id: BtwId::new(),
            at: message,
            question: question.clone(),
            answer: String::new(),
            model: live.s.model.clone(),
            asked_at_ms: now,
            added: false,
        };
        self.emit(ChatEvent::BtwStarted {
            session: id,
            id: entry.id,
            message,
            question,
        });
        let (client, tx) = (self.client.clone(), self.run_tx.clone());
        tokio::spawn(async move {
            let result = answer_once(client, body).await;
            let _ = tx.send(RunMsg::Btw {
                session: id,
                entry,
                result,
            });
        });
    }

    /// Appends a kept side question to the conversation. Only when idle.
    async fn btw_add(&mut self, id: SessionId, entry: BtwId) {
        if self
            .sessions
            .get(&id)
            .is_some_and(|l| !matches!(l.slot, Slot::Idle))
        {
            self.session_error(Some(id), "Wait for the current reply to finish.".into());
            return;
        }
        self.edit(id, move |s| {
            if s.add_btw_to_chat(entry, now_ms()) {
                Ok(())
            } else {
                Err(SessionError::UnknownMessage)
            }
        })
        .await;
        if let Some(live) = self.sessions.get(&id) {
            self.emit(ChatEvent::Opened(Box::new(live.s.clone())));
        }
    }

    /// Starts a new chat from `session`'s history through `message`.
    async fn branch(&mut self, id: SessionId, message: MessageId, model: Option<String>) {
        let was_loaded = self.sessions.contains_key(&id);
        if let Err(e) = self.ensure(id).await {
            self.session_error(Some(id), load_error_text(&e));
            return;
        }
        let result = self
            .sessions
            .get(&id)
            .map(|l| l.s.branch(message, model.as_deref(), now_ms()));
        if !was_loaded
            && self
                .sessions
                .get(&id)
                .is_some_and(|l| matches!(l.slot, Slot::Idle))
        {
            self.sessions.remove(&id);
        }
        match result {
            Some(Ok(b)) => {
                self.save(&b);
                self.emit(ChatEvent::Opened(Box::new(b.clone())));
                self.sessions.insert(b.id, Live::new(b));
                self.emit_rows().await;
            }
            Some(Err(e)) => self.session_error(Some(id), e.to_string()),
            None => {}
        }
    }

    /// Compares a branch's original with the fingerprint taken when the
    /// branch was made.
    async fn check_branch(&mut self, id: SessionId) {
        let Some((origin, wanted)) = self
            .sessions
            .get(&id)
            .and_then(|l| l.s.parent.zip(l.s.origin_hash))
        else {
            return;
        };
        let current = match self.sessions.get(&origin.session_id) {
            Some(l) => Some(l.s.prefix_hash(origin.message_id)),
            None => match self.writer.load(origin.session_id).await {
                Ok(s) => Some(s.prefix_hash(origin.message_id)),
                Err(_) => None,
            },
        };
        let state = match current {
            None => BranchState::Deleted,
            Some(h) if h == Some(wanted) => BranchState::Unchanged,
            Some(_) => BranchState::Changed,
        };
        self.emit(ChatEvent::BranchChecked { session: id, state });
    }

    /// Shows another version of a fork. Only while the chat is idle.
    async fn switch_version(&mut self, id: SessionId, after: Option<MessageId>, to: usize) {
        if self
            .sessions
            .get(&id)
            .is_some_and(|l| !matches!(l.slot, Slot::Idle))
        {
            self.session_error(Some(id), "Wait for the current reply to finish.".into());
            return;
        }
        self.edit(id, move |s| {
            let before = s.updated_at_ms;
            if s.switch_version(after, to) {
                s.updated_at_ms = before;
                Ok(())
            } else {
                Err(SessionError::UnknownMessage)
            }
        })
        .await;
        if let Some(live) = self.sessions.get(&id) {
            self.emit(ChatEvent::Opened(Box::new(live.s.clone())));
        }
    }

    /// Edits a session (loading it if needed), saves it and refreshes rows.
    pub(super) async fn edit<F>(&mut self, id: SessionId, f: F)
    where
        F: FnOnce(&mut Session) -> Result<(), SessionError>,
    {
        let was_loaded = self.sessions.contains_key(&id);
        if let Err(e) = self.ensure(id).await {
            self.session_error(Some(id), load_error_text(&e));
            return;
        }
        let mut failed = None;
        if let Some(live) = self.sessions.get_mut(&id) {
            match f(&mut live.s) {
                Ok(()) => {
                    if live.s.lifetime == SessionLifetime::Persisted {
                        self.writer.save(live.s.clone());
                    }
                }
                Err(e) => failed = Some(e.to_string()),
            }
            if !was_loaded && matches!(live.slot, Slot::Idle) {
                self.sessions.remove(&id);
            }
        }
        if let Some(m) = failed {
            self.session_error(Some(id), m);
        }
        self.emit_rows().await;
    }

    async fn set_model(&mut self, id: SessionId, model: String) {
        if let Some(live) = self.sessions.get(&id) {
            if !matches!(live.slot, Slot::Idle) {
                self.session_error(
                    Some(id),
                    "Stop the current reply before changing the model.".into(),
                );
                return;
            }
        }
        self.edit(id, move |s| {
            s.model = model;
            Ok(())
        })
        .await;
    }

    async fn delete(&mut self, id: SessionId) {
        let mut incognito = false;
        if let Some(live) = self.sessions.remove(&id) {
            incognito = live.s.lifetime == SessionLifetime::Incognito;
            self.halt(live).await;
        }
        if !incognito {
            self.writer.delete(id);
        }
        self.emit_rows().await;
    }

    /// Tells a run to stop and waits briefly for its task to finish.
    async fn halt(&self, live: Live) {
        // The run task owns no session state and the manager ignores
        // anything it sends later, so it can simply be cut off.
        match live.slot {
            Slot::Running(run) => {
                let _ = run.stop.send(true);
                run.join.abort();
            }
            Slot::Tools(tool) => {
                tool.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                if let Some(join) = tool.join {
                    join.abort();
                }
            }
            _ => {}
        }
    }

    async fn begin(&mut self, id: SessionId, intent: Intent) {
        if !self.sessions.contains_key(&id) && self.ensure(id).await.is_err() {
            self.emit(ChatEvent::SendRejected {
                session: id,
                reason: SendRejection::UnknownSession,
            });
            return;
        }
        let Some(live) = self.sessions.get(&id) else {
            return;
        };
        if !matches!(live.slot, Slot::Idle) {
            // A running agent takes new messages as steering.
            let steerable =
                live.agent.is_some() && matches!(live.slot, Slot::Running(_) | Slot::Tools(_));
            if let (true, Intent::Send(text)) = (steerable, &intent) {
                if !text.trim().is_empty() {
                    self.steer(id, text.trim().to_string()).await;
                    return;
                }
            }
            self.emit(ChatEvent::SendRejected {
                session: id,
                reason: SendRejection::Busy,
            });
            return;
        }
        match &intent {
            Intent::Send(text) if text.trim().is_empty() => {
                self.emit(ChatEvent::SendRejected {
                    session: id,
                    reason: SendRejection::EmptyMessage,
                });
                return;
            }
            Intent::Retry => {
                let retryable = matches!(
                    live.s.messages.last().map(|m| &m.content),
                    Some(MessageContent::Assistant {
                        status: AssistantStatus::Failed
                            | AssistantStatus::Cancelled
                            | AssistantStatus::Interrupted,
                        ..
                    })
                );
                if !retryable {
                    self.session_error(Some(id), "There is nothing to retry.".into());
                    return;
                }
            }
            Intent::Regenerate => {
                let m = &live.s.messages;
                let ok = live.s.mode == SessionMode::Chat
                    && m.len() >= 2
                    && matches!(
                        m.last().map(|m| &m.content),
                        Some(MessageContent::Assistant {
                            status: AssistantStatus::Complete,
                            ..
                        })
                    )
                    && matches!(
                        m.get(m.len() - 2).map(|m| &m.content),
                        Some(MessageContent::User { .. })
                    );
                if !ok {
                    self.session_error(Some(id), "There is no reply to regenerate.".into());
                    return;
                }
            }
            Intent::Edit(message, text) => {
                let yours = live.s.messages.iter().any(|m| {
                    m.id == *message
                        && matches!(
                            m.content,
                            MessageContent::User {
                                delivery: UserDelivery::Normal,
                                ..
                            }
                        )
                });
                if live.s.mode != SessionMode::Chat || !yours || text.trim().is_empty() {
                    self.session_error(Some(id), "That message cannot be edited.".into());
                    return;
                }
            }
            Intent::Send(_) => {}
        }
        let model = live.s.model.clone();
        if model.trim().is_empty() {
            self.emit(ChatEvent::SendRejected {
                session: id,
                reason: SendRejection::NoModel,
            });
            return;
        }
        if let Some(info) = self.ctx_cache.get(&model).copied() {
            self.proceed(id, intent, Some(info)).await;
            return;
        }
        self.next_token += 1;
        let token = self.next_token;
        if let Some(live) = self.sessions.get_mut(&id) {
            live.slot = Slot::Preparing { token, intent };
        }
        let (client, tx) = (self.client.clone(), self.run_tx.clone());
        tokio::spawn(async move {
            let info = client.show(&model).await.map(model_info);
            let _ = tx.send(RunMsg::Shown {
                session: id,
                token,
                info,
            });
        });
    }

    /// The model's context length is known (or unknown): trim, record the
    /// user message, and start the reply.
    pub(super) async fn proceed(&mut self, id: SessionId, intent: Intent, info: Option<ModelInfo>) {
        if self
            .sessions
            .get(&id)
            .is_some_and(|l| l.s.mode == SessionMode::Agent)
        {
            self.agent_proceed(id, intent, info).await;
            return;
        }
        let show_ctx = info.and_then(|i| i.context);
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let now = now_ms();
        let before = live.s.updated_at_ms;
        // Regenerate and Edit start a new version. Check that the request
        // fits on a copy first, so a refusal leaves the chat untouched.
        let probe: Option<Vec<Message>> = match &intent {
            Intent::Regenerate => {
                let keep = live.s.messages.len().saturating_sub(1);
                Some(live.s.messages[..keep].to_vec())
            }
            Intent::Edit(message, text) => {
                let at = live
                    .s
                    .messages
                    .iter()
                    .position(|m| m.id == *message)
                    .unwrap_or(0);
                let mut v = live.s.messages[..at].to_vec();
                v.push(Message {
                    id: MessageId::new(),
                    created_at_ms: now,
                    content: MessageContent::User {
                        text: text.trim().to_string(),
                        delivery: UserDelivery::Normal,
                    },
                });
                Some(v)
            }
            _ => None,
        };
        let user = match &intent {
            Intent::Send(text) => {
                let uid = live.s.push_user(text, now);
                live.s.messages.iter().find(|m| m.id == uid).cloned()
            }
            _ => None,
        };
        let win = window(live.s.model_options.num_ctx, show_ctx);
        let source = probe.as_deref().unwrap_or(&live.s.messages);
        let trimmed = match build_request(CHAT_SYSTEM_PROMPT, source, win) {
            Ok(t) => t,
            Err(e) => {
                if user.is_some() {
                    live.s.messages.pop();
                    live.s.updated_at_ms = before;
                }
                live.slot = Slot::Idle;
                let close = live.close_when_idle;
                self.emit(ChatEvent::SendRejected {
                    session: id,
                    reason: SendRejection::TooLarge(e.to_string()),
                });
                if close {
                    self.sessions.remove(&id);
                }
                return;
            }
        };
        let new_version = probe.is_some();
        match &intent {
            Intent::Regenerate => {
                let at = live.s.messages.len().saturating_sub(1);
                live.s.fork_at(at);
            }
            Intent::Edit(message, text) => {
                let at = live
                    .s
                    .messages
                    .iter()
                    .position(|m| m.id == *message)
                    .unwrap_or(0);
                live.s.fork_at(at);
                live.s.push_user(text.trim(), now);
            }
            _ => {}
        }
        let (run_id, turn) = (RunId::new(), TurnId::new());
        let assistant = live.s.begin_assistant(run_id, turn, now);
        let body = ChatRequestBody {
            model: live.s.model.clone(),
            messages: trimmed.messages,
            num_ctx: win,
            temperature: live.s.model_options.temperature,
            seed: live.s.model_options.seed,
            think: live.s.model_options.think,
            tools: None,
        };
        let (stop_tx, stop_rx) = watch::channel(false);
        let join = tokio::spawn(drive(
            self.client.clone(),
            body,
            id,
            turn,
            self.run_tx.clone(),
            stop_rx,
        ));
        live.slot = Slot::Running(Box::new(Run {
            turn,
            assistant,
            stop: stop_tx,
            join,
            content: String::new(),
            thinking: String::new(),
            pending_since: None,
        }));
        live.last_flush = Instant::now();
        let session = live.s.clone();
        let assistant_msg = session.messages.iter().find(|m| m.id == assistant).cloned();
        self.save(&session);
        if new_version {
            // The messages changed shape: send the whole chat.
            self.emit(ChatEvent::Opened(Box::new(session)));
        } else {
            if let Some(m) = user {
                self.emit(ChatEvent::Appended {
                    session: id,
                    message: Box::new(m),
                });
            }
            if let Some(m) = assistant_msg {
                self.emit(ChatEvent::Appended {
                    session: id,
                    message: Box::new(m),
                });
            }
        }
        self.emit(ChatEvent::TurnStarted { session: id, turn });
        if trimmed.omitted > 0 {
            self.emit(ChatEvent::ContextTrimmed {
                session: id,
                omitted: trimmed.omitted,
            });
        }
        let others_running = self
            .sessions
            .iter()
            .any(|(k, l)| *k != id && matches!(l.slot, Slot::Running(_) | Slot::Tools(_)));
        if others_running {
            self.emit(ChatEvent::WaitingForModel { session: id, turn });
        }
        self.emit_rows().await;
    }

    async fn stop(&mut self, id: SessionId) {
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        match live.slot {
            Slot::Running(_) | Slot::Tools(_) => self.stop_run(id).await,
            Slot::Preparing { .. } => {
                live.slot = Slot::Idle;
                self.emit(ChatEvent::SendRejected {
                    session: id,
                    reason: SendRejection::Cancelled,
                });
            }
            Slot::Idle => {}
        }
    }

    async fn on_run_msg(&mut self, msg: RunMsg) {
        match msg {
            RunMsg::Shown {
                session,
                token,
                info,
            } => {
                let Some(live) = self.sessions.get_mut(&session) else {
                    return;
                };
                let Slot::Preparing {
                    token: t,
                    ref intent,
                } = live.slot
                else {
                    return;
                };
                if t != token {
                    return;
                }
                let intent = intent.clone();
                live.slot = Slot::Idle;
                let model = live.s.model.clone();
                let info = match info {
                    Ok(i) => {
                        self.ctx_cache.insert(model, i);
                        Some(i)
                    }
                    Err(_) => None,
                };
                self.proceed(session, intent, info).await;
            }
            RunMsg::Btw {
                session,
                mut entry,
                result,
            } => match result {
                Ok(answer) => {
                    entry.answer = answer;
                    let kept = entry.clone();
                    self.edit(session, move |s| {
                        let before = s.updated_at_ms;
                        s.push_btw(entry);
                        s.updated_at_ms = before;
                        Ok(())
                    })
                    .await;
                    self.emit(ChatEvent::BtwDone {
                        session,
                        entry: kept,
                    });
                }
                Err(message) => self.emit(ChatEvent::BtwFailed {
                    session,
                    id: entry.id,
                    message,
                }),
            },
            RunMsg::Capabilities { model, info } => {
                if let Ok(i) = info {
                    self.ctx_cache.insert(model.clone(), i);
                    self.emit(ChatEvent::ModelCapabilities {
                        model,
                        tools: i.tools,
                    });
                }
            }
            RunMsg::ToolDone {
                session,
                run,
                call_id,
                result,
                change,
            } => self.tool_done(session, run, call_id, result, change).await,
            RunMsg::Event {
                session,
                turn,
                event,
            } => {
                let Some(live) = self.sessions.get_mut(&session) else {
                    return;
                };
                let Slot::Running(run) = &mut live.slot else {
                    return;
                };
                if run.turn != turn {
                    return;
                }
                match event {
                    StreamEvent::Content(c) => {
                        live.dirty = true;
                        live.s.append_assistant(run.assistant, &c, "");
                        run.content.push_str(&c);
                    }
                    StreamEvent::Thinking(t) => {
                        live.dirty = true;
                        live.s.append_assistant(run.assistant, "", &t);
                        run.thinking.push_str(&t);
                    }
                    StreamEvent::ToolCall { name, arguments } => {
                        if let Some(agent) = live.agent.as_mut() {
                            agent.collect(name, arguments);
                        }
                    }
                    StreamEvent::Done {
                        prompt_eval_count,
                        done_reason,
                        stats,
                        ..
                    } => {
                        live.s.set_reply_stats(run.assistant, stats);
                        // A reply that hit the window or output limit is cut
                        // off, not finished: keep the text, offer Retry.
                        let (outcome, message) = if done_reason.as_deref() == Some("length") {
                            (
                                TurnOutcome::Failed,
                                Some(
                                    "The reply was cut off because it reached the length limit."
                                        .to_string(),
                                ),
                            )
                        } else {
                            (TurnOutcome::Completed, None)
                        };
                        if self
                            .sessions
                            .get(&session)
                            .is_some_and(|l| l.agent.is_some())
                        {
                            self.agent_step_done(session, outcome, message, prompt_eval_count)
                                .await;
                        } else {
                            self.finalize(session, outcome, message, prompt_eval_count)
                                .await;
                        }
                        return;
                    }
                }
                if run.pending_since.is_none() {
                    run.pending_since = Some(Instant::now());
                }
            }
            RunMsg::Failed {
                session,
                turn,
                error,
            } => {
                let current = matches!(
                    self.sessions.get(&session).map(|l| &l.slot),
                    Some(Slot::Running(r)) if r.turn == turn
                );
                if !current {
                    return;
                }
                if let Error::ModelNotFound(m) = &error {
                    self.emit(ChatEvent::ModelMissing {
                        session,
                        model: m.clone(),
                    });
                }
                self.finalize(session, TurnOutcome::Failed, Some(error_text(&error)), None)
                    .await;
            }
        }
    }

    pub(super) fn flush_pending(&self, id: SessionId, run: &mut Run) {
        if run.content.is_empty() && run.thinking.is_empty() {
            run.pending_since = None;
            return;
        }
        self.emit(ChatEvent::Delta {
            session: id,
            turn: run.turn,
            content: std::mem::take(&mut run.content),
            thinking: std::mem::take(&mut run.thinking),
        });
        run.pending_since = None;
    }

    /// Ends a chat reply: the turn's outcome is also the run's.
    pub(super) async fn finalize(
        &mut self,
        id: SessionId,
        outcome: TurnOutcome,
        message: Option<String>,
        prompt_eval_count: Option<u64>,
    ) {
        if !self
            .finalize_turn(id, outcome, message.clone(), prompt_eval_count)
            .await
        {
            return;
        }
        let run = match outcome {
            TurnOutcome::Completed => RunOutcome::Completed,
            TurnOutcome::Failed => RunOutcome::Failed,
            TurnOutcome::Cancelled | TurnOutcome::Interrupted => RunOutcome::Cancelled,
        };
        self.end_run(id, run, message).await;
    }

    /// Records one model turn's single terminal outcome. Anything the run
    /// task sends afterwards is ignored because the slot is no longer
    /// running. False if there was no streaming turn to end.
    pub(super) async fn finalize_turn(
        &mut self,
        id: SessionId,
        outcome: TurnOutcome,
        message: Option<String>,
        prompt_eval_count: Option<u64>,
    ) -> bool {
        let Some(live) = self.sessions.get_mut(&id) else {
            return false;
        };
        let Slot::Running(mut run) = std::mem::replace(&mut live.slot, Slot::Idle) else {
            return false;
        };
        let _ = run.stop.send(true);
        let (status, error) = match outcome {
            TurnOutcome::Completed => (AssistantStatus::Complete, None),
            TurnOutcome::Failed => (AssistantStatus::Failed, message.clone()),
            TurnOutcome::Cancelled => (AssistantStatus::Cancelled, None),
            TurnOutcome::Interrupted => (AssistantStatus::Interrupted, None),
        };
        live.s
            .finish_assistant(run.assistant, status, error, now_ms());
        let s = live.s.clone();
        self.flush_pending(id, &mut run);
        self.save(&s);
        self.emit(ChatEvent::TurnEnded {
            session: id,
            turn: run.turn,
            outcome,
            message,
            prompt_eval_count,
        });
        true
    }

    /// Ends a run: one `RunEnded`, then the session may be unloaded.
    pub(super) async fn end_run(
        &mut self,
        id: SessionId,
        outcome: RunOutcome,
        message: Option<String>,
    ) {
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        live.slot = Slot::Idle;
        live.agent = None;
        let close = live.close_when_idle;
        // An agent keeps how its run ended, so the reason outlives the window.
        let kept = (live.s.mode == SessionMode::Agent).then(|| {
            live.s.last_run_end = super::types::run_end_note(
                outcome,
                message.as_deref(),
                live.s.step_budget,
                live.s.messages.len(),
            );
            live.s.clone()
        });
        if let Some(s) = kept {
            self.save(&s);
        }
        self.emit(ChatEvent::RunEnded {
            session: id,
            outcome,
            message,
        });
        if close {
            self.sessions.remove(&id);
        }
        self.emit_rows().await;
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.sessions
            .values()
            .filter_map(|l| match &l.slot {
                Slot::Running(r) => {
                    let flush = l.last_flush + self.flush_interval;
                    Some(match r.pending_since {
                        Some(p) => flush.min(p + self.coalesce),
                        None => flush,
                    })
                }
                _ => None,
            })
            .min()
    }

    fn on_tick(&mut self) {
        let now = Instant::now();
        let (coalesce, flush_every) = (self.coalesce, self.flush_interval);
        let ids: Vec<SessionId> = self.sessions.keys().copied().collect();
        for id in ids {
            let mut deltas = None;
            let mut save = None;
            if let Some(l) = self.sessions.get_mut(&id) {
                if let Slot::Running(r) = &mut l.slot {
                    if r.pending_since.is_some_and(|p| now >= p + coalesce) {
                        deltas = Some((
                            r.turn,
                            std::mem::take(&mut r.content),
                            std::mem::take(&mut r.thinking),
                        ));
                        r.pending_since = None;
                    }
                    if now >= l.last_flush + flush_every {
                        l.last_flush = now;
                        if std::mem::take(&mut l.dirty) {
                            save = Some(l.s.clone());
                        }
                    }
                }
            }
            if let Some((turn, content, thinking)) = deltas {
                self.emit(ChatEvent::Delta {
                    session: id,
                    turn,
                    content,
                    thinking,
                });
            }
            if let Some(s) = save {
                self.save(&s);
            }
        }
    }

    async fn shutdown(&mut self) {
        let running: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, l)| matches!(l.slot, Slot::Running(_) | Slot::Tools(_)))
            .map(|(id, _)| *id)
            .collect();
        for id in running {
            self.stop_run(id).await;
        }
        self.sessions
            .retain(|_, l| l.s.lifetime != SessionLifetime::Incognito);
        self.writer.flush().await;
    }
}
