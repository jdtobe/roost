//! The vocabulary between the GUI and the chat manager: intent commands in,
//! ordered events out.

use crate::session::{
    BtwEntry, BtwId, Message, MessageId, Permission, SessionId, SessionLifetime, TurnId,
};
use crate::store::IndexRow;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum ChatCommand {
    Create {
        lifetime: SessionLifetime,
        model: String,
    },
    Open(SessionId),
    /// View-level for saved sessions; destroys an incognito one.
    Close(SessionId),
    ClearIncognito,
    /// An agent: a chat with tools. The workspace folder can be attached
    /// later; until then a file tool waits for one (`WorkspaceNeeded`).
    CreateAgent {
        lifetime: SessionLifetime,
        model: String,
        workspace: Option<PathBuf>,
    },
    /// Ask whether a model advertises tool support (answered by `ModelCapabilities`).
    CheckModel(String),
    /// Change an idle agent's workspace folder.
    SetWorkspace(SessionId, PathBuf),
    /// How many model steps an agent run may use (1 to 100).
    SetStepBudget(SessionId, u32),
    SetToolEnabled(SessionId, String, bool),
    /// The app-wide switch for agents writing files (Settings). Off makes
    /// every agent read-only, whatever its own configuration says.
    SetWritesAllowed(bool),
    /// Settings: how much the agent tools may read, search, write and run.
    SetLimits(Box<crate::agent::tools::Limits>),
    /// The app-wide switch for agents running commands (Settings, off by
    /// default). Off hides the command tool from every agent.
    SetRunAllowed(bool),
    /// Puts a file an agent changed back as it was. Refused if the file was
    /// edited since or a run is active.
    UndoChange {
        session: SessionId,
        change: u32,
    },
    /// Undoes every change, newest first, stopping at one that cannot be.
    UndoAll(SessionId),
    SetCapability(SessionId, String, Permission),
    Rename(SessionId, String),
    SetPinned(SessionId, bool),
    /// Deletes a session. A saved one is kept out of sight in the store's
    /// trash until `PurgeDeleted` (or the next start), so `Undelete` can bring
    /// it back; an incognito one is simply dropped.
    Delete(SessionId),
    /// Brings back a session deleted a moment ago.
    Undelete(SessionId),
    /// Removes a deleted session for good (its Undo time is over).
    PurgeDeleted(SessionId),
    SetModel(SessionId, String),
    Send {
        session: SessionId,
        text: String,
    },
    Stop(SessionId),
    Retry(SessionId),
    /// Park the last complete reply as a version and ask again (chats only).
    Regenerate(SessionId),
    /// Replace one of your messages with new text as a new version, and ask
    /// again from there (chats only).
    Edit {
        session: SessionId,
        message: MessageId,
        text: String,
    },
    /// Start a new chat from the history through a finished reply, on
    /// `model` or the same one; the new chat opens.
    Branch {
        session: SessionId,
        message: MessageId,
        model: Option<String>,
    },
    /// Ask a side question about the conversation up to `message`. The
    /// question and answer are kept apart from the transcript.
    Btw {
        session: SessionId,
        message: MessageId,
        question: String,
    },
    /// Append a side question and its answer to the conversation.
    BtwAdd {
        session: SessionId,
        id: BtwId,
    },
    /// Forget a side question.
    BtwDelete {
        session: SessionId,
        id: BtwId,
    },
    /// Compare a branch's original with what the branch was made from; answered
    /// with `BranchChecked`.
    CheckBranch(SessionId),
    /// The answer to an `ApprovalRequested` event.
    ResolveApproval {
        session: SessionId,
        call_id: String,
        decision: ApprovalDecision,
    },
    /// The answer to a `WorkspaceNeeded` event: the folder to attach, or
    /// `None` to go on without one (the call fails).
    ProvideWorkspace {
        session: SessionId,
        call_id: String,
        path: Option<PathBuf>,
    },
    /// The answer to a `QuestionAsked` event. `None` is "skip".
    AnswerQuestion {
        session: SessionId,
        call_id: String,
        answer: Option<crate::agent::ask::Answer>,
    },
    /// Show another version of a fork (`after` is the message before it).
    SwitchVersion {
        session: SessionId,
        after: Option<MessageId>,
        to: usize,
    },
    ListSessions,
    /// The installed models changed: forget cached context lengths.
    InstalledChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendRejection {
    /// A reply is already being produced.
    Busy,
    /// Readable explanation of why the message cannot fit.
    TooLarge(String),
    UnknownSession,
    EmptyMessage,
    /// Stopped while preparing, before anything was sent.
    Cancelled,
    /// The model does not advertise tool support, so it cannot run an agent.
    NoToolSupport(String),
    /// The agent's workspace folder is missing or cannot be opened.
    NoWorkspace(String),
    /// No model is chosen yet (an agent can be opened before one is picked).
    NoModel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Failed,
    Cancelled,
    /// The user steered an agent: the partial reply is kept.
    Interrupted,
}

/// What to remember of how a run ended, if it did not finish normally.
/// `after` is the number of messages the chat had then.
pub fn run_end_note(
    outcome: RunOutcome,
    message: Option<&str>,
    step_budget: u32,
    after: usize,
) -> Option<crate::session::RunEndNote> {
    use crate::session::{RunEnd, RunEndNote};
    let end = match outcome {
        RunOutcome::Completed => return None,
        RunOutcome::BudgetExhausted => RunEnd::StepLimit { steps: step_budget },
        RunOutcome::Cancelled => RunEnd::Stopped,
        RunOutcome::Failed => RunEnd::Failed {
            message: message.unwrap_or_default().to_string(),
        },
        RunOutcome::TextToolCall => RunEnd::TextToolCall {
            tool: message.unwrap_or_default().to_string(),
        },
    };
    Some(RunEndNote { end, after })
}

/// How a whole run (one chat reply, or an agent's loop of steps) ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Failed,
    Cancelled,
    /// An agent used all its steps.
    BudgetExhausted,
    /// The last reply was a tool call written as text; nothing ran. The
    /// message names the tool or tools.
    TextToolCall,
}

#[derive(Debug, Clone)]
pub enum ChatEvent {
    /// Every session row, persisted and incognito.
    Rows(Vec<IndexRow>),
    /// A session's transcript, after create or open.
    Opened(Box<crate::session::Session>),
    /// A message added to a session (the user's, then the empty reply).
    Appended {
        session: SessionId,
        message: Box<Message>,
    },
    TurnStarted {
        session: SessionId,
        turn: TurnId,
    },
    /// Another reply is being generated, so this one may be queued.
    WaitingForModel {
        session: SessionId,
        turn: TurnId,
    },
    Delta {
        session: SessionId,
        turn: TurnId,
        content: String,
        thinking: String,
    },
    ContextTrimmed {
        session: SessionId,
        omitted: usize,
    },
    /// Exactly one per turn. `message` is the readable failure, if any.
    TurnEnded {
        session: SessionId,
        turn: TurnId,
        outcome: TurnOutcome,
        message: Option<String>,
        prompt_eval_count: Option<u64>,
    },
    SendRejected {
        session: SessionId,
        reason: SendRejection,
    },
    /// An agent started a tool call.
    ToolStarted {
        session: SessionId,
        call_id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ToolFinished {
        session: SessionId,
        call_id: String,
        ok: bool,
        output: String,
    },
    /// A message sent to a running agent, waiting for its next safe step.
    SteeringQueued {
        session: SessionId,
        text: String,
    },
    /// Exactly one per run, after its last `TurnEnded`.
    RunEnded {
        session: SessionId,
        outcome: RunOutcome,
        message: Option<String>,
    },
    ModelCapabilities {
        model: String,
        tools: bool,
    },
    /// An idle agent's folder, step budget or tool settings changed.
    AgentSettings {
        session: SessionId,
        workspace: Option<PathBuf>,
        step_budget: u32,
        tools: crate::session::EffectiveToolConfig,
    },
    /// The model is not installed; the user should pick another.
    ModelMissing {
        session: SessionId,
        model: String,
    },
    /// A tool set to Ask wants to run; the run waits for `ResolveApproval`.
    ApprovalRequested {
        session: SessionId,
        call_id: String,
        name: String,
        arguments: serde_json::Value,
        /// For a write, what it would change.
        preview: Option<crate::agent::change::ChangePreview>,
        /// For a command, exactly what will run and where.
        run: Option<crate::agent::command::RunPreview>,
    },
    /// A tool call needs a working directory and the agent has none; the run
    /// waits for `ProvideWorkspace`.
    WorkspaceNeeded {
        session: SessionId,
        call_id: String,
        name: String,
        arguments: serde_json::Value,
    },
    /// The agent asks a question and waits for `AnswerQuestion`.
    QuestionAsked {
        session: SessionId,
        call_id: String,
        question: crate::agent::ask::Question,
    },
    /// The files an agent has changed in this chat, after any change.
    Changes {
        session: SessionId,
        changes: Vec<crate::session::ChangeRecord>,
    },
    /// An undo that could not be done, and why.
    UndoRefused {
        session: SessionId,
        change: u32,
        reason: String,
    },
    /// A side question was sent.
    BtwStarted {
        session: SessionId,
        id: BtwId,
        message: MessageId,
        question: String,
    },
    BtwDone {
        session: SessionId,
        entry: BtwEntry,
    },
    BtwFailed {
        session: SessionId,
        id: BtwId,
        message: String,
    },
    /// A side question was forgotten.
    BtwDeleted {
        session: SessionId,
        id: BtwId,
    },
    /// How a branch's original compares with when the branch was made.
    BranchChecked {
        session: SessionId,
        state: BranchState,
    },
    StoreWarning(String),
    SessionError {
        session: Option<SessionId>,
        message: String,
    },
}

/// What the user answered to a tool approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Run this call.
    Allow,
    /// Run this call and stop asking for this kind of tool for the rest of the session.
    AllowForSession,
    /// Do not run it; the model is told.
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchState {
    /// The original's history up to the branch point is as it was.
    Unchanged,
    /// The original's history up to the branch point is different now.
    Changed,
    /// The original chat no longer exists.
    Deleted,
}

pub type EventSink = Arc<dyn Fn(ChatEvent) + Send + Sync>;
