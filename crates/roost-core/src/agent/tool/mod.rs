//! One `impl Tool` per tool: what it is, how a call is planned and how it
//! runs. The registry decides which tools may run; a tool decides nothing
//! about permission.

use super::change;
use super::registry::{Options, ToolSpec};
use super::tools::Limits;
use super::workspace::{ToolError, Workspace};
use serde_json::Value;

/// What the chat loop should do with a call before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Run it now: it changes nothing and needs no say from the user.
    Run,
    /// Show the user what it will do and wait for a yes.
    Approve {
        scope: Scope,
        preview: Option<Preview>,
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
    /// Everything the tool's capability covers.
    Capability,
    File(String),
    CommandPrefix(String),
    Host(String),
}

/// What a write will change, shown on the approval card. Not used yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview(pub String);

/// The command a run will start, shown on the approval card. Not used yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPreview(pub String);

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
}

pub struct RunCx<'a> {
    pub workspace: &'a Workspace,
    pub limits: &'a Limits,
    pub opts: &'a Options<'a>,
}

pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// True when the tool cannot run without a workspace folder.
    fn needs_workspace(&self) -> bool;
    fn options(&self) -> &'static [OptionDecl];
    fn plan(&self, cx: &PlanCx, args: &Value) -> Plan;
    fn run(
        &self,
        cx: &RunCx,
        args: &Value,
    ) -> (Result<String, ToolError>, Option<change::Change>);
}

/// The tools defined through the trait.
pub fn all() -> &'static [&'static dyn Tool] {
    &[]
}
