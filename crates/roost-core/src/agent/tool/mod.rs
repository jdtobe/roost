//! One `impl Tool` per tool: what it is, how a call is planned and how it
//! runs. The registry decides which tools may run; a tool decides nothing
//! about permission.

use super::change::{self, ChangePreview};
use super::command::RunPreview;
use super::registry::{Options, ToolSpec};
use super::tools::Limits;
use super::workspace::{ToolError, Workspace};
use serde_json::Value;

mod ask;
mod files_read;
mod files_write;
mod shell;
mod web;

/// What the chat loop should do with a call before it runs. The tool's
/// permission still decides: under Auto an `Approve` runs without asking,
/// and under Off nothing runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Run it without asking, even under Ask: a call that cannot be planned
    /// changes nothing, and running it lets the model read why.
    Run,
    /// Under Ask, show the user what it will do and wait for a yes, unless
    /// `scope` was already allowed for this chat.
    Approve {
        scope: Scope,
        /// What a write will change, shown on the approval card.
        preview: Option<ChangePreview>,
        /// The command a run will start, shown on the approval card.
        run: Option<RunPreview>,
    },
    /// Put a question to the user and take the answer as the result.
    Question,
    /// Refuse without running.
    Refuse(ToolError),
}

/// What "Allow for this chat" remembers, and what an earlier allowance covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Everything the tool's capability covers: allowing it sets the
    /// capability to Auto.
    Capability,
    /// One workspace-relative file, as the approval card shows it.
    File(String),
    /// Plain commands that start with this prefix, such as `cargo test`.
    CommandPrefix(String),
    /// Nothing is remembered: allowing covers only this call (a command no
    /// prefix can safely cover).
    Once,
    /// One web host, lowercased, such as `example.com`.
    Host(String),
}

/// Where a call runs: on the blocking pool, or awaited on the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exec {
    Blocking,
    Network,
}

/// The plan of a tool with nothing to show: under Ask it asks, and "Allow
/// for this chat" allows its whole capability.
pub fn by_capability() -> Plan {
    Plan::Approve {
        scope: Scope::Capability,
        preview: None,
        run: None,
    }
}

/// A limit the user can lower for a session, declared by the tool that
/// reads it. The ceiling is `get` on the app-wide limits.
pub struct OptionDecl {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub min: u64,
    pub get: fn(&Limits) -> u64,
    pub set: fn(&mut Limits, u64),
}

pub struct PlanCx<'a> {
    pub workspace: Option<&'a Workspace>,
    pub limits: &'a Limits,
    /// Questions this run has already asked the user.
    pub questions_asked: u32,
}

pub struct RunCx<'a> {
    pub workspace: &'a Workspace,
    pub limits: &'a Limits,
    pub opts: &'a Options<'a>,
}

pub trait Tool: Send + Sync {
    /// The name the model calls it by; the same as `spec().name`, without
    /// building the schema.
    fn name(&self) -> &'static str;
    fn spec(&self) -> ToolSpec;
    /// True when the tool cannot run without a workspace folder.
    fn needs_workspace(&self) -> bool;
    /// How the chat loop runs a call.
    fn exec(&self) -> Exec;
    fn options(&self) -> &'static [OptionDecl];
    fn plan(&self, cx: &PlanCx, args: &Value) -> Plan;
    fn run(&self, cx: &RunCx, args: &Value) -> (Result<String, ToolError>, Option<change::Change>);
}

/// Every built-in tool, in the order the model is told about them.
pub fn all() -> &'static [&'static dyn Tool] {
    &[
        &files_read::ListDir,
        &files_read::ReadFile,
        &files_read::ReadBinary,
        &files_read::FindFiles,
        &files_read::SearchText,
        &files_read::SearchBytes,
        &files_write::WriteFile,
        &files_write::EditFile,
        &shell::RunCommand,
        &ask::AskUser,
        &web::FetchUrl,
    ]
}

/// The tool called `name`, if there is one.
pub fn find(name: &str) -> Option<&'static dyn Tool> {
    all().iter().copied().find(|t| t.name() == name)
}
