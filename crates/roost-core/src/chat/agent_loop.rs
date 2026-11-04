//! Agent runs: the model/tool step loop, steering and the step budget.
//!
//! A run is a series of steps. Each step streams one model reply; if the reply
//! asks for tools they run one at a time and the loop goes again, until the
//! model answers without tools, the user stops it, or the steps run out.
//! Steering messages wait in an inbox and are applied at the next safe point.

use super::context::{build_agent_request, window};
use super::manager::{now_ms, Intent, Manager, ModelInfo, Run, Slot, CHAT_SYSTEM_PROMPT};
use super::run::{drive, RunMsg};
use super::types::{ApprovalDecision, ChatEvent, RunOutcome, SendRejection, TurnOutcome};
use crate::agent::ask;
use crate::agent::change::{self, ChangePreview};
use crate::agent::command::{self, RunPreview};
use crate::agent::registry;
use crate::agent::tool::{self, Exec, Plan, PlanCx, Scope};
use crate::agent::tools::Limits;
use crate::agent::web;
use crate::agent::{ToolError, Workspace};
use crate::ollama::ChatRequestBody;
use crate::session::{
    EffectiveToolConfig, MessageId, Permission, RunId, Session, SessionId, SessionLifetime,
    SessionMode, ToolStatus, TurnId,
};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

pub const AGENT_PROMPT_ADDITION: &str = "You are operating as an agent inside the attached workspace. Use only the provided tools, read a file before you change it, stay within the workspace, use fetch_url (not shell commands such as curl) to read web pages when it is offered, never invent tool results, honor steering messages, and stop when the task is complete or the step budget is exhausted.";

#[derive(Clone)]
struct PendingCall {
    call_id: String,
    name: String,
    arguments: Value,
    msg: MessageId,
}

/// The loop state of one active agent run.
pub(super) struct AgentRun {
    run: RunId,
    steps: u32,
    /// Tool calls the current reply has asked for so far.
    collected: Vec<(String, Value)>,
    queue: VecDeque<PendingCall>,
    inbox: VecDeque<String>,
    /// `None` until the user attaches a folder.
    workspace: Option<Arc<Workspace>>,
    next_call: u32,
    /// Questions this run has asked the user.
    asked: u32,
}

impl AgentRun {
    /// What the tool called `name` says must happen before the call runs;
    /// `None` when there is no such tool.
    pub(super) fn plan(&self, name: &str, arguments: &Value, lim: &Limits) -> Option<Plan> {
        let cx = PlanCx {
            workspace: self.workspace.as_deref(),
            limits: lim,
            questions_asked: self.asked,
        };
        Some(tool::find(name)?.plan(&cx, arguments))
    }

    /// What a write call would change, if `name` is a write tool and the call
    /// makes sense.
    pub(super) fn preview(
        &self,
        name: &str,
        arguments: &Value,
        lim: &Limits,
    ) -> Option<ChangePreview> {
        match self.plan(name, arguments, lim)? {
            Plan::Approve { preview, .. } => preview,
            _ => None,
        }
    }

    /// What a command call would run and where, if the tool plans one.
    pub(super) fn run_preview(
        &self,
        name: &str,
        arguments: &Value,
        lim: &Limits,
    ) -> Option<RunPreview> {
        match self.plan(name, arguments, lim)? {
            Plan::Approve { run, .. } => run,
            _ => None,
        }
    }

    /// Whether a working directory is attached.
    pub(super) fn has_workspace(&self) -> bool {
        self.workspace.is_some()
    }

    pub(super) fn collect(&mut self, name: String, arguments: Value) {
        self.collected.push((name, arguments));
    }
}

/// A tool call running on the blocking pool.
pub(super) struct ToolRun {
    /// The running task; `None` while the call waits for the user's answer.
    pub(super) join: Option<JoinHandle<()>>,
    call_id: String,
    msg: MessageId,
    /// The call, while it waits for approval.
    waiting: Option<PendingCall>,
    /// The waiting call is a question for the user, not an approval.
    question: bool,
    /// Set to end a running command; a blocking task cannot be aborted.
    pub(super) cancel: Arc<AtomicBool>,
}

impl ToolRun {
    /// The call waiting for the user's answer, if that is what this is.
    pub(super) fn waiting_call(&self) -> Option<(String, String, Value)> {
        self.waiting
            .as_ref()
            .map(|c| (c.call_id.clone(), c.name.clone(), c.arguments.clone()))
    }
}

/// A session's tool configuration with the app-wide switches applied: with
/// writing or commands off in Settings, that capability is off whatever the
/// session says.
fn gated(
    config: &EffectiveToolConfig,
    writes_allowed: bool,
    commands_allowed: bool,
) -> EffectiveToolConfig {
    let mut c = config.clone();
    if !writes_allowed {
        c.capabilities
            .insert("files.write".to_string(), Permission::Off);
    }
    if !commands_allowed {
        c.capabilities
            .insert("shell.run".to_string(), Permission::Off);
    }
    c
}

/// The system prompt for one agent step, with the workspace, the tools that
/// are on and the steps left appended as structured context.
fn system_prompt(s: &Session, tools: &EffectiveToolConfig, steps_left: u32) -> String {
    let tools: Vec<&str> = registry::offered(tools).iter().map(|t| t.name).collect();
    let workspace = s.workspace.as_ref().map_or_else(
        || {
            "none attached (a file or command tool will ask the user to choose a folder)"
                .to_string()
        },
        |p| p.display().to_string(),
    );
    format!(
        "{CHAT_SYSTEM_PROMPT} {AGENT_PROMPT_ADDITION}\n\nWorkspace: {workspace}\nTools: {}\nSteps remaining: {steps_left}",
        if tools.is_empty() {
            "none".to_string()
        } else {
            tools.join(", ")
        }
    )
}

impl Manager {
    // ------------------------------------------------------------ creating

    pub(super) async fn create_agent(
        &mut self,
        lifetime: SessionLifetime,
        model: &str,
        workspace: Option<PathBuf>,
    ) {
        if let Some(Err(e)) = workspace.as_deref().map(Workspace::open) {
            self.session_error(None, format!("That folder cannot be used: {e}"));
            return;
        }
        let s = match Session::new_agent(model, lifetime, workspace, now_ms()) {
            Ok(s) => s,
            Err(e) => {
                self.session_error(None, e.to_string());
                return;
            }
        };
        self.save(&s);
        self.emit(ChatEvent::Opened(Box::new(s.clone())));
        self.sessions.insert(s.id, super::manager::Live::new(s));
        self.emit_rows().await;
    }

    /// Answers `CheckModel` from the cache, or asks Ollama.
    pub(super) fn check_model(&mut self, model: String) {
        if let Some(info) = self.ctx_cache.get(&model) {
            self.emit(ChatEvent::ModelCapabilities {
                tools: info.tools,
                model,
            });
            return;
        }
        let (client, tx) = (self.client.clone(), self.run_tx.clone());
        tokio::spawn(async move {
            let info = client.show(&model).await.map(|d| ModelInfo {
                context: d.context,
                tools: d.capabilities.iter().any(|c| c == "tools"),
            });
            let _ = tx.send(RunMsg::Capabilities { model, info });
        });
    }

    pub(super) async fn set_workspace(&mut self, id: SessionId, path: PathBuf) {
        if let Err(e) = Workspace::open(&path) {
            self.session_error(Some(id), format!("That folder cannot be used: {e}"));
            return;
        }
        self.edit_idle(id, move |s| s.set_workspace(path)).await;
    }

    /// Edits settings that must not change under a running agent.
    pub(super) async fn edit_idle<F>(&mut self, id: SessionId, f: F)
    where
        F: FnOnce(&mut Session) -> Result<(), crate::session::SessionError>,
    {
        if self
            .sessions
            .get(&id)
            .is_some_and(|l| !matches!(l.slot, Slot::Idle))
        {
            self.session_error(
                Some(id),
                "Stop the current run before changing this.".into(),
            );
            return;
        }
        self.edit(id, f).await;
        if let Some(live) = self
            .sessions
            .get(&id)
            .filter(|l| l.s.mode == SessionMode::Agent)
        {
            self.emit(ChatEvent::AgentSettings {
                session: id,
                workspace: live.s.workspace.clone(),
                step_budget: live.s.step_budget,
                tools: live.s.effective_tools.clone(),
            });
        }
    }

    // ------------------------------------------------------------ starting

    /// The model's capabilities are known: check the preconditions, record
    /// the user's message and start the loop.
    pub(super) async fn agent_proceed(
        &mut self,
        id: SessionId,
        intent: Intent,
        info: Option<ModelInfo>,
    ) {
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let reject = |this: &Self, reason| {
            this.emit(ChatEvent::SendRejected {
                session: id,
                reason,
            })
        };
        if let Some(i) = info {
            if !i.tools {
                let model = live.s.model.clone();
                live.slot = Slot::Idle;
                reject(self, SendRejection::NoToolSupport(model));
                return;
            }
        }
        let opened = match live.s.workspace.as_deref().map(Workspace::open) {
            Some(Ok(w)) => Some(Arc::new(w)),
            Some(Err(e)) => {
                live.slot = Slot::Idle;
                reject(self, SendRejection::NoWorkspace(e.to_string()));
                return;
            }
            // The agent can start; a tool that needs a folder asks for one.
            None => None,
        };
        let user = match &intent {
            Intent::Send(text) => {
                let uid = live.s.push_user(text, now_ms());
                live.s.messages.iter().find(|m| m.id == uid).cloned()
            }
            Intent::Retry | Intent::Regenerate | Intent::Edit(..) => None,
        };
        live.agent = Some(AgentRun {
            run: RunId::new(),
            steps: 0,
            collected: Vec::new(),
            queue: VecDeque::new(),
            inbox: VecDeque::new(),
            workspace: opened,
            next_call: 0,
            asked: 0,
        });
        let s = live.s.clone();
        self.save(&s);
        if let Some(m) = user {
            self.emit(ChatEvent::Appended {
                session: id,
                message: Box::new(m),
            });
        }
        self.agent_step(id).await;
    }

    /// Streams one model reply for the run.
    async fn agent_step(&mut self, id: SessionId) {
        let (writes, runs) = (self.writes_allowed, self.commands_allowed);
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let Some(agent) = live.agent.as_mut() else {
            return;
        };
        agent.steps += 1;
        let (run_id, steps_before) = (agent.run, agent.steps - 1);
        let left = live.s.step_budget.saturating_sub(steps_before);
        let config = gated(&live.s.effective_tools, writes, runs);
        let system = system_prompt(&live.s, &config, left);
        let context = self.ctx_cache.get(&live.s.model).and_then(|i| i.context);
        let win = window(live.s.model_options.num_ctx, context);
        let trimmed = match build_agent_request(&system, &live.s.messages, win) {
            Ok(t) => t,
            Err(e) => {
                let message = e.to_string();
                self.end_run(id, RunOutcome::Failed, Some(message)).await;
                return;
            }
        };
        let turn = TurnId::new();
        let assistant = live.s.begin_assistant(run_id, turn, now_ms());
        let tools = registry::ollama_definitions(&config);
        let body = ChatRequestBody {
            model: live.s.model.clone(),
            messages: trimmed.messages,
            num_ctx: win,
            temperature: live.s.model_options.temperature,
            seed: live.s.model_options.seed,
            think: live.s.model_options.think,
            tools: tools
                .as_array()
                .is_some_and(|a| !a.is_empty())
                .then_some(tools),
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
        if let Some(m) = assistant_msg {
            self.emit(ChatEvent::Appended {
                session: id,
                message: Box::new(m),
            });
        }
        self.emit(ChatEvent::TurnStarted { session: id, turn });
        if trimmed.omitted > 0 {
            self.emit(ChatEvent::ContextTrimmed {
                session: id,
                omitted: trimmed.omitted,
            });
        }
        self.emit_rows().await;
    }

    // ------------------------------------------------------------- stepping

    /// A reply finished streaming. Run its tools, or finish the run.
    pub(super) async fn agent_step_done(
        &mut self,
        id: SessionId,
        outcome: TurnOutcome,
        message: Option<String>,
        prompt_eval_count: Option<u64>,
    ) {
        let calls = self
            .sessions
            .get_mut(&id)
            .and_then(|l| l.agent.as_mut())
            .map(|a| std::mem::take(&mut a.collected))
            .unwrap_or_default();
        if outcome == TurnOutcome::Failed {
            // Cut off by the length limit: keep the text, end the run.
            self.finalize(id, outcome, message, prompt_eval_count).await;
            return;
        }
        if !self
            .finalize_turn(id, outcome, message, prompt_eval_count)
            .await
        {
            return;
        }
        if calls.is_empty() {
            let steering = self
                .sessions
                .get(&id)
                .and_then(|l| l.agent.as_ref())
                .is_some_and(|a| !a.inbox.is_empty());
            if steering {
                self.agent_next(id).await;
            } else {
                // A reply that is only a tool call written as text did not
                // finish anything: say so instead of calling it an answer.
                let known: Vec<&str> = registry::specs().iter().map(|s| s.name).collect();
                let written = self
                    .sessions
                    .get(&id)
                    .and_then(|l| {
                        l.s.messages.iter().rev().find_map(|m| match &m.content {
                            crate::session::MessageContent::Assistant { text, .. } => Some(
                                crate::agent::text_call::tool_calls_written_as_text(text, &known),
                            ),
                            _ => None,
                        })
                    })
                    .unwrap_or_default();
                if written.is_empty() {
                    self.end_run(id, RunOutcome::Completed, None).await;
                } else {
                    self.end_run(id, RunOutcome::TextToolCall, Some(written.join(", ")))
                        .await;
                }
            }
            return;
        }
        // Record every call first so the transcript keeps them together.
        let mut recorded = Vec::new();
        if let Some(live) = self.sessions.get_mut(&id) {
            if let Some(agent) = live.agent.as_mut() {
                for (name, arguments) in calls {
                    agent.next_call += 1;
                    let call_id = format!("call_{}", agent.next_call);
                    let msg = live.s.push_tool_call(
                        agent.run,
                        &call_id,
                        &name,
                        arguments.clone(),
                        now_ms(),
                    );
                    if let Some(m) = live.s.messages.iter().find(|m| m.id == msg).cloned() {
                        recorded.push(m);
                    }
                    agent.queue.push_back(PendingCall {
                        call_id,
                        name,
                        arguments,
                        msg,
                    });
                }
            }
            let s = live.s.clone();
            self.save(&s);
        }
        for m in recorded {
            self.emit(ChatEvent::Appended {
                session: id,
                message: Box::new(m),
            });
        }
        self.run_next_tool(id).await;
    }

    /// Starts the next queued tool call, or moves on when none are left.
    async fn run_next_tool(&mut self, id: SessionId) {
        let (writes, runs) = (self.writes_allowed, self.commands_allowed);
        let lim = self.limits_for(id);
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let Some(agent) = live.agent.as_mut() else {
            return;
        };
        let Some(call) = agent.queue.pop_front() else {
            self.agent_next(id).await;
            return;
        };
        if !agent.has_workspace() && registry::needs_workspace(&call.name) {
            self.need_workspace(id, call);
            return;
        }
        let config = gated(&live.s.effective_tools, writes, runs);
        let plan = match agent.plan(&call.name, &call.arguments, &lim) {
            Some(Plan::Question) => {
                self.ask_question(id, call);
                return;
            }
            Some(Plan::Refuse(e)) => {
                // A tool that is off says so first, as it would if it ran.
                let e = registry::authorize(&config, &call.name, true)
                    .err()
                    .unwrap_or(e);
                self.refuse(id, call, e);
                return;
            }
            plan => plan,
        };
        let mut ask = registry::needs_approval(&config, &call.name);
        if ask {
            match &plan {
                // A call that cannot be planned changes nothing; running it
                // lets the model read why.
                Some(Plan::Run) => ask = false,
                // The user allowed edits to this file for the session.
                Some(Plan::Approve {
                    scope: Scope::File(path),
                    ..
                }) if live.allowed_files.contains(path) => ask = false,
                // An allowed prefix covers a plain command that starts with
                // it and nothing chained to it.
                Some(Plan::Approve {
                    scope: Scope::CommandPrefix(own),
                    ..
                }) if live
                    .allowed_prefixes
                    .iter()
                    .any(|p| command::covered_by(own, p)) =>
                {
                    ask = false
                }
                // The user allowed this web host for the session.
                Some(Plan::Approve {
                    scope: Scope::Host(host),
                    ..
                }) if live.allowed_hosts.contains(host) => ask = false,
                _ => {}
            }
        }
        let (preview, run) = match plan {
            Some(Plan::Approve { preview, run, .. }) => (preview, run),
            _ => (None, None),
        };
        if ask {
            // Ask the user first; the run waits in this slot.
            live.slot = Slot::Tools(Box::new(ToolRun {
                join: None,
                call_id: call.call_id.clone(),
                msg: call.msg,
                waiting: Some(call.clone()),
                question: false,
                cancel: Arc::new(AtomicBool::new(false)),
            }));
            self.emit(ChatEvent::ApprovalRequested {
                session: id,
                call_id: call.call_id,
                name: call.name,
                arguments: call.arguments,
                preview,
                run,
            });
            return;
        }
        let approved = registry::needs_approval(&config, &call.name);
        self.start_tool_call(id, call, approved);
    }

    /// The tool limits for this chat's model: the Settings ceilings, with the
    /// size of one result cut to fit the model's context window.
    pub(super) fn limits_for(&self, id: SessionId) -> crate::agent::tools::Limits {
        let window = self.sessions.get(&id).map(|live| {
            let context = self.ctx_cache.get(&live.s.model).and_then(|i| i.context);
            window(live.s.model_options.num_ctx, context)
        });
        match window {
            Some(w) => self.limits.for_window(w),
            None => self.limits.clone(),
        }
    }

    /// Runs a tool call on the blocking pool. `approved` lets a tool set to
    /// Ask through.
    fn start_tool_call(&mut self, id: SessionId, call: PendingCall, approved: bool) {
        let (writes, runs) = (self.writes_allowed, self.commands_allowed);
        let lim = self.limits_for(id);
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let Some(agent) = live.agent.as_mut() else {
            return;
        };
        live.s.start_tool(call.msg);
        let (workspace, config, run, room) = (
            agent.workspace.clone(),
            gated(&live.s.effective_tools, writes, runs),
            agent.run,
            live.s.undo_room(lim.undo_bytes),
        );
        let (tx, name, args, call_id) = (
            self.run_tx.clone(),
            call.name.clone(),
            call.arguments.clone(),
            call.call_id.clone(),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancel);
        let network = tool::find(&name).is_some_and(|t| t.exec() == Exec::Network);
        let join = if network {
            // The network is awaited, not run on the blocking pool.
            let max = lim.output_bytes;
            tokio::spawn(async move {
                let result = fetch_for_agent(&config, &name, &args, approved, &stop, max).await;
                let _ = tx.send(RunMsg::ToolDone {
                    session: id,
                    run,
                    call_id,
                    result,
                    change: None,
                });
            })
        } else if let Some(workspace) = workspace {
            tokio::task::spawn_blocking(move || {
                let (result, change) = registry::call_tracked(
                    &workspace,
                    &config,
                    &name,
                    &args,
                    &lim,
                    &registry::Options {
                        approved,
                        undo_room: room,
                        cancel: &stop,
                    },
                );
                let _ = tx.send(RunMsg::ToolDone {
                    session: id,
                    run,
                    call_id,
                    result,
                    change,
                });
            })
        } else {
            // Only reachable for a call that is not a known tool; it fails
            // like any call that cannot run.
            tokio::spawn(async move {
                let _ = tx.send(RunMsg::ToolDone {
                    session: id,
                    run,
                    call_id,
                    result: Err(ToolError::NoWorkspace(name)),
                    change: None,
                });
            })
        };
        live.slot = Slot::Tools(Box::new(ToolRun {
            join: Some(join),
            call_id: call.call_id.clone(),
            msg: call.msg,
            waiting: None,
            question: false,
            cancel,
        }));
        self.emit(ChatEvent::ToolStarted {
            session: id,
            call_id: call.call_id,
            name: call.name,
            arguments: call.arguments,
        });
    }

    /// The user answered a tool approval.
    pub(super) async fn resolve_approval(
        &mut self,
        id: SessionId,
        call_id: String,
        decision: ApprovalDecision,
    ) {
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let call = match &mut live.slot {
            // A question is answered with `AnswerQuestion`, not an approval.
            Slot::Tools(t) if t.call_id == call_id && !t.question => t.waiting.take(),
            _ => None,
        };
        let Some(call) = call else {
            return;
        };
        match decision {
            ApprovalDecision::Allow => self.start_tool_call(id, call, true),
            ApprovalDecision::AllowForSession => {
                let lim = self.limits_for(id);
                let plan = self
                    .sessions
                    .get(&id)
                    .and_then(|l| l.agent.as_ref())
                    .and_then(|a| a.plan(&call.name, &call.arguments, &lim));
                match plan {
                    // Writing never becomes automatic: only this file is allowed.
                    Some(Plan::Approve {
                        scope: Scope::File(path),
                        ..
                    }) => {
                        if let Some(live) = self.sessions.get_mut(&id) {
                            live.allowed_files.insert(path);
                        }
                    }
                    // Commands are never allowed wholesale: only a plain prefix
                    // such as `cargo test`.
                    Some(Plan::Approve {
                        scope: Scope::CommandPrefix(prefix),
                        ..
                    }) => {
                        if let Some(live) = self.sessions.get_mut(&id) {
                            live.allowed_prefixes.insert(prefix);
                        }
                    }
                    // The web never becomes automatic: only this host is
                    // allowed, for this chat.
                    Some(Plan::Approve {
                        scope: Scope::Host(host),
                        ..
                    }) => {
                        if let Some(live) = self.sessions.get_mut(&id) {
                            live.allowed_hosts.insert(host);
                        }
                    }
                    Some(Plan::Approve {
                        scope: Scope::Capability,
                        ..
                    }) => {
                        if let (Some(key), Some(live)) = (
                            registry::capability_of(&call.name),
                            self.sessions.get_mut(&id),
                        ) {
                            live.s.set_capability(&key, Permission::Auto);
                            let s = live.s.clone();
                            self.save(&s);
                        }
                    }
                    // A command no prefix can cover (risky programs, chained
                    // commands): only this call is allowed.
                    Some(Plan::Approve {
                        scope: Scope::Once, ..
                    }) => {}
                    // Nothing to remember (a call that no longer plans):
                    // only this call is allowed.
                    _ => {}
                }
                self.start_tool_call(id, call, true);
            }
            ApprovalDecision::Deny => {
                // Reported to the model like any failed call.
                self.tool_done(
                    id,
                    self.current_run(id),
                    call.call_id,
                    Err(ToolError::Declined),
                    None,
                )
                .await;
            }
        }
    }

    /// A call needs a working directory and none is attached: the run waits
    /// in this slot until `ProvideWorkspace`. A tool that is switched off
    /// fails as usual instead, without bothering the user.
    fn need_workspace(&mut self, id: SessionId, call: PendingCall) {
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let Some(agent) = live.agent.as_ref() else {
            return;
        };
        let run = agent.run;
        let usable = registry::specs()
            .iter()
            .find(|sp| sp.name == call.name)
            .is_some_and(|sp| {
                matches!(
                    registry::check(&live.s.effective_tools, sp),
                    Ok(()) | Err(ToolError::NeedsApproval(_))
                )
            });
        live.slot = Slot::Tools(Box::new(ToolRun {
            join: None,
            call_id: call.call_id.clone(),
            msg: call.msg,
            waiting: usable.then(|| call.clone()),
            question: false,
            cancel: Arc::new(AtomicBool::new(false)),
        }));
        if usable {
            self.emit(ChatEvent::WorkspaceNeeded {
                session: id,
                call_id: call.call_id,
                name: call.name,
                arguments: call.arguments,
            });
        } else {
            // Finished by the manager loop, like a call that ran elsewhere.
            let _ = self.run_tx.send(RunMsg::ToolDone {
                session: id,
                run,
                call_id: call.call_id,
                result: Err(ToolError::Off(call.name)),
                change: None,
            });
        }
    }

    /// The user answered `WorkspaceNeeded`: attach the folder and run the
    /// waiting call, or (`None`) fail it.
    pub(super) async fn provide_workspace(
        &mut self,
        id: SessionId,
        call_id: String,
        path: Option<PathBuf>,
    ) {
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let waiting = match &live.slot {
            Slot::Tools(t) if t.call_id == call_id => t
                .waiting
                .clone()
                .filter(|c| registry::needs_workspace(&c.name)),
            _ => None,
        };
        let Some(call) =
            waiting.filter(|_| live.agent.as_ref().is_some_and(|a| !a.has_workspace()))
        else {
            return;
        };
        let Some(path) = path else {
            if let Slot::Tools(t) = &mut live.slot {
                t.waiting = None;
            }
            let name = call.name.clone();
            self.tool_done(
                id,
                self.current_run(id),
                call.call_id,
                Err(ToolError::NoWorkspace(name)),
                None,
            )
            .await;
            return;
        };
        let opened = match Workspace::open(&path) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.session_error(Some(id), format!("That folder cannot be used: {e}"));
                return;
            }
        };
        if let Err(e) = live.s.set_workspace(path) {
            self.session_error(Some(id), e.to_string());
            return;
        }
        if let Some(agent) = live.agent.as_mut() {
            agent.workspace = Some(opened);
            agent.queue.push_front(call);
        }
        live.slot = Slot::Idle;
        let s = live.s.clone();
        self.save(&s);
        self.emit(ChatEvent::AgentSettings {
            session: id,
            workspace: s.workspace.clone(),
            step_budget: s.step_budget,
            tools: s.effective_tools.clone(),
        });
        self.run_next_tool(id).await;
    }

    /// The model asks the user a question: the run waits in this slot until
    /// `AnswerQuestion`. A question to a tool that is switched off goes back
    /// to the model as a failed call instead (the tool's plan refuses one
    /// past the run's limit, or one it cannot read).
    fn ask_question(&mut self, id: SessionId, call: PendingCall) {
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let on = tool::find(&call.name)
            .is_some_and(|t| registry::check(&live.s.effective_tools, &t.spec()).is_ok());
        let asked = if on {
            ask::parse(&call.arguments).map_err(ToolError::InvalidArguments)
        } else {
            Err(ToolError::Off(call.name.clone()))
        };
        let question = match asked {
            Ok(q) => q,
            Err(e) => {
                self.refuse(id, call, e);
                return;
            }
        };
        let Some(agent) = live.agent.as_mut() else {
            return;
        };
        agent.asked += 1;
        live.slot = Slot::Tools(Box::new(ToolRun {
            join: None,
            call_id: call.call_id.clone(),
            msg: call.msg,
            waiting: Some(call.clone()),
            question: true,
            cancel: Arc::new(AtomicBool::new(false)),
        }));
        self.emit(ChatEvent::QuestionAsked {
            session: id,
            call_id: call.call_id,
            question,
        });
    }

    /// Fails a call without running it or asking the user; the model reads
    /// why. Finished by the manager loop, like a call that ran elsewhere.
    fn refuse(&mut self, id: SessionId, call: PendingCall, e: ToolError) {
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let Some(agent) = live.agent.as_ref() else {
            return;
        };
        let run = agent.run;
        live.slot = Slot::Tools(Box::new(ToolRun {
            join: None,
            call_id: call.call_id.clone(),
            msg: call.msg,
            waiting: None,
            question: false,
            cancel: Arc::new(AtomicBool::new(false)),
        }));
        let _ = self.run_tx.send(RunMsg::ToolDone {
            session: id,
            run,
            call_id: call.call_id,
            result: Err(e),
            change: None,
        });
    }

    /// The user answered a question (`None` is skip).
    pub(super) async fn answer_question(
        &mut self,
        id: SessionId,
        call_id: String,
        answer: Option<ask::Answer>,
    ) {
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let call = match &live.slot {
            Slot::Tools(t) if t.call_id == call_id && t.question => t.waiting.clone(),
            _ => None,
        };
        let Some(call) = call else {
            return;
        };
        let Ok(question) = ask::parse(&call.arguments) else {
            return;
        };
        let text = match answer {
            None => ask::SKIPPED.to_string(),
            Some(a) => match ask::check_answer(&question, &a) {
                Ok(()) => ask::result_text(&question, &a),
                Err(_) => {
                    // Not a valid answer: the window still has the question.
                    self.emit(ChatEvent::QuestionAsked {
                        session: id,
                        call_id,
                        question,
                    });
                    return;
                }
            },
        };
        if let Some(Slot::Tools(t)) = self.sessions.get_mut(&id).map(|l| &mut l.slot) {
            t.waiting = None;
        }
        self.tool_done(id, self.current_run(id), call.call_id, Ok(text), None)
            .await;
    }

    fn current_run(&self, id: SessionId) -> RunId {
        self.sessions
            .get(&id)
            .and_then(|l| l.agent.as_ref())
            .map_or_else(RunId::new, |a| a.run)
    }

    pub(super) async fn tool_done(
        &mut self,
        id: SessionId,
        run: RunId,
        call_id: String,
        result: Result<String, ToolError>,
        change: Option<change::Change>,
    ) {
        // A write that happened is kept for undo even if the run was stopped
        // meanwhile and nobody is waiting for the answer any more.
        if let Some(ch) = &change {
            self.record_change(id, &call_id, ch);
        }
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let msg = match &live.slot {
            Slot::Tools(t) if t.call_id == call_id => t.msg,
            _ => return,
        };
        if live.agent.as_ref().map(|a| a.run) != Some(run) {
            return;
        }
        live.slot = Slot::Idle;
        let (ok, output) = match result {
            Ok(text) => (true, text),
            Err(e) => (false, format!("Error: {e}")),
        };
        let status = if ok {
            ToolStatus::Complete
        } else {
            ToolStatus::Failed
        };
        live.s.finish_tool(msg, &output, status, now_ms());
        let s = live.s.clone();
        self.save(&s);
        self.emit(ChatEvent::ToolFinished {
            session: id,
            call_id,
            ok,
            output,
        });
        self.run_next_tool(id).await;
    }

    /// Keeps a change for undo, saves it and tells the window.
    fn record_change(&mut self, id: SessionId, call_id: &str, ch: &change::Change) {
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let msg = match &live.slot {
            Slot::Tools(t) if t.call_id == call_id => t.msg,
            _ => MessageId::new(),
        };
        live.s.record_change(msg, ch);
        let (s, changes) = (live.s.clone(), live.s.changes.clone());
        self.save(&s);
        self.emit(ChatEvent::Changes {
            session: id,
            changes,
        });
    }

    /// Puts changed files back as they were: one change, or all of them
    /// newest first. A file edited since is left alone, and nothing is undone
    /// while the agent is working.
    pub(super) fn undo(&mut self, id: SessionId, only: Option<u32>) {
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        let refuse = |this: &mut Self, change: u32, reason: &str| {
            this.emit(ChatEvent::UndoRefused {
                session: id,
                change,
                reason: reason.to_string(),
            });
        };
        let ids: Vec<u32> = match only {
            Some(one) => vec![one],
            None => live
                .s
                .changes
                .iter()
                .rev()
                .filter(|c| !c.undone)
                .map(|c| c.id)
                .collect(),
        };
        if !matches!(live.slot, Slot::Idle) {
            let first = ids.first().copied().unwrap_or(0);
            refuse(
                self,
                first,
                "Stop the agent first; changes can be undone when it is idle.",
            );
            return;
        }
        let Some(root) = live.s.workspace.clone() else {
            refuse(
                self,
                ids.first().copied().unwrap_or(0),
                "This chat has no folder.",
            );
            return;
        };
        let ws = match Workspace::open(&root) {
            Ok(w) => w,
            Err(e) => {
                refuse(self, ids.first().copied().unwrap_or(0), &e.to_string());
                return;
            }
        };
        let mut changed = false;
        for cid in ids {
            let Some(live) = self.sessions.get_mut(&id) else {
                return;
            };
            let Some(rec) = live.s.changes.iter().find(|c| c.id == cid).cloned() else {
                refuse(self, cid, "That change is not in this chat.");
                break;
            };
            if rec.undone {
                refuse(self, cid, "That change was already undone.");
                break;
            }
            if rec.root != root {
                refuse(
                    self,
                    cid,
                    &format!(
                        "This change was made in {}, not in the folder the agent works in now.",
                        rec.root.display()
                    ),
                );
                break;
            }
            let rel = std::path::PathBuf::from(&rec.path);
            let now = match ws.read_text_for_edit(&rel, self.limits.edit_file_bytes) {
                Ok(Some(t)) => t,
                Ok(None) => {
                    refuse(
                        self,
                        cid,
                        &format!(
                            "{} is gone, so there is nothing to put back over.",
                            rec.path
                        ),
                    );
                    break;
                }
                Err(e) => {
                    refuse(self, cid, &e.to_string());
                    break;
                }
            };
            if change::fingerprint(&now) != rec.after_hash {
                refuse(
                    self,
                    cid,
                    &format!("{} was changed after the agent wrote it, so it was left alone. Undo later changes first, or restore it yourself.", rec.path),
                );
                break;
            }
            let done = if rec.created {
                ws.remove_file(&rel)
            } else {
                ws.write_text(&rel, &rec.before)
            };
            if let Err(e) = done {
                refuse(self, cid, &e.to_string());
                break;
            }
            if let Some(live) = self.sessions.get_mut(&id) {
                if let Some(c) = live.s.changes.iter_mut().find(|c| c.id == cid) {
                    c.undone = true;
                    c.before.clear();
                }
            }
            changed = true;
        }
        if changed {
            if let Some(live) = self.sessions.get(&id) {
                let (s, changes) = (live.s.clone(), live.s.changes.clone());
                self.save(&s);
                self.emit(ChatEvent::Changes {
                    session: id,
                    changes,
                });
            }
        }
    }

    /// A safe point (no reply streaming, no tool running): apply waiting
    /// steering messages, then go again unless the steps are used up.
    async fn agent_next(&mut self, id: SessionId) {
        let mut appended = Vec::new();
        let (steps, budget) = {
            let Some(live) = self.sessions.get_mut(&id) else {
                return;
            };
            let Some(agent) = live.agent.as_mut() else {
                return;
            };
            while let Some(text) = agent.inbox.pop_front() {
                let mid = live.s.push_steering(&text, now_ms());
                if let Some(m) = live.s.messages.iter().find(|m| m.id == mid).cloned() {
                    appended.push(m);
                }
            }
            (agent.steps, live.s.step_budget)
        };
        if !appended.is_empty() {
            if let Some(live) = self.sessions.get(&id) {
                let s = live.s.clone();
                self.save(&s);
            }
        }
        for m in appended {
            self.emit(ChatEvent::Appended {
                session: id,
                message: Box::new(m),
            });
        }
        if steps >= budget {
            let message = format!("Stopped after using all {budget} steps.");
            self.end_run(id, RunOutcome::BudgetExhausted, Some(message))
                .await;
            return;
        }
        self.agent_step(id).await;
    }

    // ------------------------------------------------------------- steering

    /// The user sent a message to a running agent. It waits for the next
    /// safe point; a reply that is streaming is interrupted (and kept).
    pub(super) async fn steer(&mut self, id: SessionId, text: String) {
        let streaming = {
            let Some(live) = self.sessions.get_mut(&id) else {
                return;
            };
            let Some(agent) = live.agent.as_mut() else {
                return;
            };
            agent.inbox.push_back(text.clone());
            matches!(live.slot, Slot::Running(_))
        };
        self.emit(ChatEvent::SteeringQueued { session: id, text });
        if !streaming {
            return; // a tool is running; its result is the next safe point
        }
        if let Some(agent) = self.sessions.get_mut(&id).and_then(|l| l.agent.as_mut()) {
            agent.collected.clear();
        }
        if self
            .finalize_turn(id, TurnOutcome::Interrupted, None, None)
            .await
        {
            self.agent_next(id).await;
        }
    }

    // ------------------------------------------------------------- stopping

    /// Stop: cancel whatever is running and end the run.
    pub(super) async fn stop_run(&mut self, id: SessionId) {
        let Some(live) = self.sessions.get_mut(&id) else {
            return;
        };
        match &live.slot {
            Slot::Running(_) => {
                if !self
                    .finalize_turn(id, TurnOutcome::Cancelled, None, None)
                    .await
                {
                    return;
                }
            }
            Slot::Tools(_) => {
                if let Slot::Tools(t) = std::mem::replace(&mut live.slot, Slot::Idle) {
                    // A running command is ended by its flag; aborting the
                    // blocking task would leave it running.
                    t.cancel.store(true, Ordering::SeqCst);
                    if let Some(join) = t.join {
                        join.abort();
                    }
                }
            }
            _ => return,
        }
        if let Some(live) = self.sessions.get_mut(&id) {
            live.s.cancel_open_tools(now_ms());
            let s = live.s.clone();
            self.save(&s);
        }
        self.end_run(id, RunOutcome::Cancelled, None).await;
    }
}

/// Runs one `fetch_url` call: checks it may run, fetches the page, and turns
/// it into the text the model gets. Ends early when `cancel` is set.
async fn fetch_for_agent(
    config: &crate::session::EffectiveToolConfig,
    name: &str,
    args: &Value,
    approved: bool,
    cancel: &AtomicBool,
    max_bytes: usize,
) -> Result<String, ToolError> {
    registry::authorize(config, name, approved)?;
    let url = web::parse(args).map_err(ToolError::InvalidArguments)?;
    let work = web::fetch(url, web::Rules::STRICT);
    let stopped = async {
        while !cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    };
    tokio::select! {
        page = work => match page {
            Ok(p) => Ok(web::result_text(&p, max_bytes)),
            Err(web::FetchError::Refused(why)) => Err(ToolError::Denied(why)),
            Err(web::FetchError::Failed(why)) => Err(ToolError::Unreadable(why)),
        },
        () = stopped => Err(ToolError::Unreadable("the fetch was stopped".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionLifetime;

    #[test]
    fn the_prompt_says_when_no_folder_is_attached() {
        let s = Session::new_agent("m", SessionLifetime::Persisted, None::<PathBuf>, 0).unwrap();
        let prompt = system_prompt(&s, &EffectiveToolConfig::default(), 5);
        assert!(prompt.contains("Workspace: none attached"), "{prompt}");
        assert!(prompt.contains("ask the user"), "{prompt}");
    }
}
