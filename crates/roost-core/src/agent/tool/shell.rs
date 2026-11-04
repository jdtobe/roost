//! The tool that runs a shell command on the user's computer.

use super::{Exec, OptionDecl, Plan, PlanCx, RunCx, Scope, Tool};
use crate::agent::change;
use crate::agent::command::{self, RunPreview};
use crate::agent::registry::{Capability, Family, ToolSpec};
use crate::agent::workspace::ToolError;
use serde_json::{json, Value};

pub struct RunCommand;

impl Tool for RunCommand {
    fn name(&self) -> &'static str {
        "run_command"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "run_command",
            family: Family::Shell,
            capability: Capability::Run,
            description: "Run a shell command on the user's computer, starting in the workspace folder, and return its exit code and output. It runs as the user and is not limited to the workspace. It stops after 2 minutes and only the first 20 KB of output is kept. The user is asked before it runs. Prefer the file tools for reading and changing files.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command line, as typed in a terminal."}
                },
                "required": ["command"]
            }),
        }
    }

    fn needs_workspace(&self) -> bool {
        true
    }

    fn exec(&self) -> Exec {
        Exec::Blocking
    }

    fn options(&self) -> &'static [OptionDecl] {
        &[]
    }

    /// Asks with the command card. A call without a command (or a folder to
    /// run in) runs and fails, so the model reads why. A command that no
    /// allowance can cover is `Scope::Once`.
    fn plan(&self, cx: &PlanCx, args: &Value) -> Plan {
        let (Some(ws), Some(command)) = (
            cx.workspace,
            args.get("command").and_then(Value::as_str).map(str::trim),
        ) else {
            return Plan::Run;
        };
        let scope = match command::allow_prefix(command) {
            Some(prefix) => Scope::CommandPrefix(prefix),
            None => Scope::Once,
        };
        Plan::Approve {
            scope,
            preview: None,
            run: Some(RunPreview {
                prefix: command::allow_prefix(command),
                command: command.to_string(),
                folder: ws.root().to_path_buf(),
                timeout_secs: cx.limits.run_timeout.as_secs(),
                output_kb: cx.limits.run_output_bytes / 1024,
            }),
        }
    }

    fn run(&self, cx: &RunCx, args: &Value) -> (Result<String, ToolError>, Option<change::Change>) {
        let Some(Value::String(command)) = args.get("command") else {
            return (
                Err(ToolError::InvalidArguments(
                    "command is required and must be a string".into(),
                )),
                None,
            );
        };
        (
            command::run(cx.workspace.root(), command, cx.limits, cx.opts.cancel),
            None,
        )
    }
}
