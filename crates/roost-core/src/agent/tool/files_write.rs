//! The tools that change files in the workspace. Each change is planned
//! first, so the approval card can show it and undo can keep the original.

use super::{Exec, OptionDecl, Plan, PlanCx, RunCx, Scope, Tool};
use crate::agent::change;
use crate::agent::registry::{Capability, Family, ToolSpec};
use crate::agent::workspace::ToolError;
use serde_json::{json, Value};

pub struct WriteFile;

impl Tool for WriteFile {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_file",
            family: Family::Files,
            capability: Capability::Write,
            description: "Create a file, or replace a whole file, in the workspace with the given text. Folders are made as needed. Prefer edit_file for a small change to an existing file. The user may be asked to approve the change.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File relative to the workspace root."},
                    "content": {"type": "string", "description": "The complete new text of the file."}
                },
                "required": ["path", "content"]
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

    fn plan(&self, cx: &PlanCx, args: &Value) -> Plan {
        plan(self.name(), cx, args)
    }

    fn run(&self, cx: &RunCx, args: &Value) -> (Result<String, ToolError>, Option<change::Change>) {
        write(self.name(), cx, args)
    }
}

pub struct EditFile;

impl Tool for EditFile {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file",
            family: Family::Files,
            capability: Capability::Write,
            description: "Change part of an existing file: replace old_text, which must appear exactly once, with new_text. Read the file first and copy old_text exactly, with enough surrounding lines to be unique. The user may be asked to approve the change.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File relative to the workspace root."},
                    "old_text": {"type": "string", "description": "The exact text to replace; it must occur once."},
                    "new_text": {"type": "string", "description": "What to put there instead (may be empty to delete)."}
                },
                "required": ["path", "old_text", "new_text"]
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

    fn plan(&self, cx: &PlanCx, args: &Value) -> Plan {
        plan(self.name(), cx, args)
    }

    fn run(&self, cx: &RunCx, args: &Value) -> (Result<String, ToolError>, Option<change::Change>) {
        write(self.name(), cx, args)
    }
}

/// Asks about the one file the call changes, with its diff. A call that
/// cannot be planned (or has no folder yet) runs and fails with the reason.
fn plan(name: &str, cx: &PlanCx, args: &Value) -> Plan {
    let Some(ws) = cx.workspace else {
        return Plan::Run;
    };
    match change::plan(ws, name, args, cx.limits) {
        Ok(Some(c)) => Plan::Approve {
            scope: Scope::File(c.path.clone()),
            preview: Some(c.card()),
            run: None,
        },
        Ok(None) | Err(_) => Plan::Run,
    }
}

/// Plans a write, checks the original can be kept, then applies it.
fn write(
    name: &str,
    cx: &RunCx,
    args: &Value,
) -> (Result<String, ToolError>, Option<change::Change>) {
    let ws = cx.workspace;
    let change = match change::plan(ws, name, args, cx.limits) {
        Ok(Some(c)) => c,
        Ok(None) => return (Err(ToolError::UnknownTool(name.to_string())), None),
        Err(e) => return (Err(e), None),
    };
    let needed = change.before.as_ref().map_or(0, String::len);
    if needed > cx.opts.undo_room {
        return (
            Err(ToolError::InvalidArguments(
                "this session cannot keep another copy for undo; nothing was changed. Ask the user to undo or start a new session".into(),
            )),
            None,
        );
    }
    match change::apply(ws, &change) {
        Ok(msg) => {
            let kept = (change.before.as_deref() != Some(change.after.as_str())).then_some(change);
            (Ok(msg), kept)
        }
        Err(e) => (Err(e), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::Limits;
    use crate::agent::workspace::Workspace;

    #[test]
    fn a_write_asks_about_its_file_and_one_that_cannot_be_planned_runs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let limits = Limits::default();
        let cx = PlanCx {
            workspace: Some(&ws),
            limits: &limits,
            questions_asked: 0,
        };
        let edit = json!({"path": "./a.txt", "old_text": "hello", "new_text": "bye"});
        match EditFile.plan(&cx, &edit) {
            Plan::Approve {
                scope,
                preview,
                run,
            } => {
                assert_eq!(scope, Scope::File("a.txt".into()));
                assert_eq!(preview.unwrap().path, "a.txt");
                assert!(run.is_none());
            }
            other => panic!("{other:?}"),
        }
        let missing = json!({"path": "a.txt", "old_text": "nope", "new_text": "x"});
        assert_eq!(EditFile.plan(&cx, &missing), Plan::Run);
        let no_folder = PlanCx {
            workspace: None,
            limits: &limits,
            questions_asked: 0,
        };
        let new = json!({"path": "n.txt", "content": "x"});
        assert_eq!(WriteFile.plan(&no_folder, &new), Plan::Run);
    }
}
