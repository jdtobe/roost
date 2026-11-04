//! The tool that asks the user a question and waits for the answer.

use super::{Exec, OptionDecl, Plan, PlanCx, RunCx, Tool};
use crate::agent::ask;
use crate::agent::change;
use crate::agent::registry::{Capability, Family, ToolSpec};
use crate::agent::workspace::ToolError;
use serde_json::{json, Value};

pub struct AskUser;

impl Tool for AskUser {
    fn name(&self) -> &'static str {
        ask::TOOL
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: ask::TOOL,
            family: Family::Ask,
            capability: Capability::Ask,
            description: "Ask the user a question and wait for the answer. Use it only when you cannot go on without a decision that is theirs to make, or a fact only they have; do not ask what you can find out with the other tools, and do not ask for passwords, keys or other secrets. kind \"one\": they pick one of the options. kind \"many\": they pick any number. kind \"text\": they type an answer. For one and many, give 2 to 8 short options; they can also type their own answer unless allow_other is false. The result is their answer, or says they chose not to answer.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "question": {"type": "string", "description": "The question, short and specific."},
                    "kind": {"type": "string", "enum": ["one", "many", "text"], "description": "one: pick one option. many: pick any number. text: type an answer."},
                    "options": {"type": "array", "items": {"type": "string"}, "description": "2 to 8 short options, for kind one or many."},
                    "allow_other": {"type": "boolean", "description": "Let the user type their own answer too (one and many). Defaults to true."}
                },
                "required": ["question", "kind"]
            }),
        }
    }

    fn needs_workspace(&self) -> bool {
        false
    }

    fn exec(&self) -> Exec {
        Exec::Blocking
    }

    fn options(&self) -> &'static [OptionDecl] {
        &[]
    }

    /// A question for the user. One past the run's limit, or one the model
    /// got wrong, is refused so the model reads why.
    fn plan(&self, cx: &PlanCx, args: &Value) -> Plan {
        if cx.questions_asked >= ask::MAX_PER_RUN {
            return Plan::Refuse(ToolError::InvalidArguments(format!(
                "you have already asked {} questions in this run; carry on without asking more",
                ask::MAX_PER_RUN
            )));
        }
        match ask::parse(args) {
            Ok(_) => Plan::Question,
            Err(e) => Plan::Refuse(ToolError::InvalidArguments(e)),
        }
    }

    /// Asked through the chat manager, which waits for the user.
    fn run(
        &self,
        _cx: &RunCx,
        _args: &Value,
    ) -> (Result<String, ToolError>, Option<change::Change>) {
        (
            Err(ToolError::InvalidArguments(
                "this tool is handled by the app, not run here".into(),
            )),
            None,
        )
    }
}
