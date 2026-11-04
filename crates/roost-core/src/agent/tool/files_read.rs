//! The tools that read files and folders in the workspace.

use super::{OptionDecl, Plan, PlanCx, RunCx, Tool};
use crate::agent::change;
use crate::agent::registry::{Capability, Family, ToolSpec};
use crate::agent::tools;
use crate::agent::workspace::ToolError;
use serde_json::{json, Value};

pub struct ListDir;

impl Tool for ListDir {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_dir",
            family: Family::Files,
            capability: Capability::Read,
            description: "List the entries of a directory in the workspace, by name or sorted by size or time. Directories end with /. A long listing is cut, and the result says which offset to use to see the rest.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory relative to the workspace root. Defaults to the root."},
                    "offset": {"type": "integer", "description": "Entries to skip, to continue a cut listing. Defaults to 0."},
                    "limit": {"type": "integer", "description": "Return at most this many (never more than the user allows). Ask for fewer when a few will do."},
                    "sort": {"type": "string", "enum": ["name", "size", "modified"], "description": "Order: name (default), size (largest first) or modified (newest first)."},
                    "reverse": {"type": "boolean", "description": "Flip the order."}
                }
            }),
        }
    }

    fn needs_workspace(&self) -> bool {
        true
    }

    fn options(&self) -> &'static [OptionDecl] {
        &[]
    }

    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Plan {
        Plan::Run
    }

    fn run(&self, cx: &RunCx, args: &Value) -> (Result<String, ToolError>, Option<change::Change>) {
        (tools::list_dir(cx.workspace, args, cx.limits), None)
    }
}

pub struct ReadFile;

impl Tool for ReadFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file",
            family: Family::Files,
            capability: Capability::Read,
            description: "Read a text file from the workspace, with line numbers. Long files are cut, and the result says which offset to use to continue.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File relative to the workspace root."},
                    "offset": {"type": "integer", "description": "First line to return, starting at 1."},
                    "limit": {"type": "integer", "description": "Number of lines to return."}
                },
                "required": ["path"]
            }),
        }
    }

    fn needs_workspace(&self) -> bool {
        true
    }

    fn options(&self) -> &'static [OptionDecl] {
        &[]
    }

    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Plan {
        Plan::Run
    }

    fn run(&self, cx: &RunCx, args: &Value) -> (Result<String, ToolError>, Option<change::Change>) {
        (tools::read_file(cx.workspace, args, cx.limits), None)
    }
}
