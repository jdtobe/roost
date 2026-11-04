//! Which tools exist and which may run. Four layers: family, capability,
//! permission and tool. The Files family has read tools and write tools.

use super::change;
use super::tools::{self, Limits};
use super::workspace::{ToolError, Workspace};
use crate::session::{EffectiveToolConfig, Permission};
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;

/// For callers that cannot be cancelled.
static NO_CANCEL: AtomicBool = AtomicBool::new(false);

/// How one tool call is run, beyond what it is.
pub struct Options<'a> {
    /// The user said yes to a tool that asks.
    pub approved: bool,
    /// Bytes of originals that may still be kept for undo.
    pub undo_room: usize,
    /// Set to end a running command.
    pub cancel: &'a AtomicBool,
}

impl Options<'static> {
    /// No undo limit and no way to cancel.
    pub fn once(approved: bool) -> Options<'static> {
        Options {
            approved,
            undo_room: usize::MAX,
            cancel: &NO_CANCEL,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Files,
    Shell,
    /// Asking the user questions.
    Ask,
    /// Reading the web.
    Web,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Read,
    Write,
    Run,
    /// Asking the user a question. Always allowed: it changes nothing.
    Ask,
    /// Fetching a page from the internet: the one thing that leaves the machine.
    Fetch,
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub family: Family,
    pub capability: Capability,
    pub description: &'static str,
    /// JSON Schema of the arguments, as Ollama expects it.
    pub parameters: Value,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Family::Files => "Files",
            Family::Shell => "Shell",
            Family::Ask => "Ask you",
            Family::Web => "Web",
        }
    }
}

impl Capability {
    pub fn label(self) -> &'static str {
        match self {
            Capability::Read => "Read",
            Capability::Write => "Write",
            Capability::Run => "Run",
            Capability::Ask => "Ask",
            Capability::Fetch => "Web",
        }
    }
}

/// The key a capability has in `EffectiveToolConfig::capabilities`.
pub fn capability_key(family: Family, capability: Capability) -> String {
    let f = match family {
        Family::Files => "files",
        Family::Shell => "shell",
        Family::Ask => "ask",
        Family::Web => "web",
    };
    let c = match capability {
        Capability::Read => "read",
        Capability::Write => "write",
        Capability::Run => "run",
        Capability::Ask => "ask",
        Capability::Fetch => "fetch",
    };
    format!("{f}.{c}")
}

/// The most a capability may be set to. Reading and writing files in the
/// agent's folder may be `Auto`: every write keeps the original for undo.
/// Whatever a session's file says, `permission_of` never goes above this.
pub fn max_permission(_family: Family, capability: Capability) -> Permission {
    match capability {
        Capability::Read
        | Capability::Write
        | Capability::Run
        | Capability::Ask
        | Capability::Fetch => Permission::Auto,
    }
}

/// What a capability is set to when the session never configured it: reading
/// is automatic, writing asks first.
pub fn default_permission(_family: Family, capability: Capability) -> Permission {
    match capability {
        Capability::Read => Permission::Auto,
        Capability::Write => Permission::Ask,
        // Running commands is off until the user turns it on for an agent.
        Capability::Run => Permission::Off,
        // Asking changes nothing, so it needs no permission.
        Capability::Ask => Permission::Auto,
        // The web is off until the user turns it on for an agent.
        Capability::Fetch => Permission::Off,
    }
}

/// The built-in tools.
pub fn specs() -> Vec<ToolSpec> {
    let read = |name, description, parameters| ToolSpec {
        name,
        family: Family::Files,
        capability: Capability::Read,
        description,
        parameters,
    };
    let write = |name, description, parameters| ToolSpec {
        name,
        family: Family::Files,
        capability: Capability::Write,
        description,
        parameters,
    };
    let run = |name, description, parameters| ToolSpec {
        name,
        family: Family::Shell,
        capability: Capability::Run,
        description,
        parameters,
    };
    vec![
        read(
            "list_dir",
            "List the entries of a directory in the workspace, by name or sorted by size or time. Directories end with /. A long listing is cut, and the result says which offset to use to see the rest.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory relative to the workspace root. Defaults to the root."},
                    "offset": {"type": "integer", "description": "Entries to skip, to continue a cut listing. Defaults to 0."},
                    "limit": {"type": "integer", "description": "Return at most this many (never more than the user allows). Ask for fewer when a few will do."},
                    "sort": {"type": "string", "enum": ["name", "size", "modified"], "description": "Order: name (default), size (largest first) or modified (newest first)."},
                    "reverse": {"type": "boolean", "description": "Flip the order."}
                }
            }),
        ),
        read(
            "read_file",
            "Read a text file from the workspace, with line numbers. Long files are cut, and the result says which offset to use to continue.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File relative to the workspace root."},
                    "offset": {"type": "integer", "description": "First line to return, starting at 1."},
                    "limit": {"type": "integer", "description": "Number of lines to return."}
                },
                "required": ["path"]
            }),
        ),
        read(
            "read_binary",
            "Show the bytes of any file (a ROM, an image, an archive …) as a hex dump: offset, 16 bytes in hex, and the same bytes as text. Returns a limited number of bytes per call and says which offset to use to continue. Offsets may be written as 4096 or 0x1000. Use this when read_file says a file is binary.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File relative to the workspace root."},
                    "offset": {"type": "integer", "description": "First byte to show, counting from 0. Defaults to 0."},
                    "length": {"type": "integer", "description": "How many bytes to show (default 256, at most 4096)."}
                },
                "required": ["path"]
            }),
        ),
        read(
            "find_files",
            "Find files and directories whose path matches a glob such as *.rs or src/*.toml. A pattern without a slash matches at any depth. Results can be sorted by size or time (to find the largest or newest files). Many results are cut, and the result says which offset to use to see the next ones.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob pattern."},
                    "path": {"type": "string", "description": "Directory to search in. Defaults to the workspace root."},
                    "offset": {"type": "integer", "description": "Results to skip, to continue a cut list. Defaults to 0."},
                    "limit": {"type": "integer", "description": "Return at most this many (never more than the user allows). Ask for fewer when a few will do."},
                    "sort": {"type": "string", "enum": ["name", "size", "modified"], "description": "Order: name (default), size (largest first) or modified (newest first)."},
                    "reverse": {"type": "boolean", "description": "Flip the order."}
                },
                "required": ["pattern"]
            }),
        ),
        read(
            "search_text",
            "Search text files for a regular expression. Returns path:line: text for each match. Many matches are cut, and the result says which offset to use to see the next ones and which files could not be searched (binary files, files too large).",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regular expression."},
                    "path": {"type": "string", "description": "File or directory to search. Defaults to the workspace root."},
                    "case_insensitive": {"type": "boolean", "description": "Ignore letter case."},
                    "offset": {"type": "integer", "description": "Matches to skip, to continue a cut list. Defaults to 0."},
                    "limit": {"type": "integer", "description": "Return at most this many (never more than the user allows). Ask for fewer when a few will do."}
                },
                "required": ["pattern"]
            }),
        ),
        read(
            "search_bytes",
            "Search files for a byte pattern, such as a ROM header or a file signature. The pattern is hex pairs, with ?? for any byte: \"4E 45 53 1A\" or \"de ?? be ef\". Returns path:0xOFFSET: bytes for each match. Use read_binary to look around an offset. Many matches are cut, and the result says which offset to use to see the next ones.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Hex byte pairs, ?? for any byte. Up to 64 bytes."},
                    "path": {"type": "string", "description": "File or directory to search. Defaults to the workspace root."},
                    "glob": {"type": "string", "description": "Only search files whose name matches, such as *.nes."},
                    "offset": {"type": "integer", "description": "Matches to skip, to continue a cut list. Defaults to 0."},
                    "limit": {"type": "integer", "description": "Return at most this many (never more than the user allows). Ask for fewer when a few will do."}
                },
                "required": ["pattern"]
            }),
        ),
        write(
            "write_file",
            "Create a file, or replace a whole file, in the workspace with the given text. Folders are made as needed. Prefer edit_file for a small change to an existing file. The user may be asked to approve the change.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File relative to the workspace root."},
                    "content": {"type": "string", "description": "The complete new text of the file."}
                },
                "required": ["path", "content"]
            }),
        ),
        write(
            "edit_file",
            "Change part of an existing file: replace old_text, which must appear exactly once, with new_text. Read the file first and copy old_text exactly, with enough surrounding lines to be unique. The user may be asked to approve the change.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File relative to the workspace root."},
                    "old_text": {"type": "string", "description": "The exact text to replace; it must occur once."},
                    "new_text": {"type": "string", "description": "What to put there instead (may be empty to delete)."}
                },
                "required": ["path", "old_text", "new_text"]
            }),
        ),
        run(
            "run_command",
            "Run a shell command on the user's computer, starting in the workspace folder, and return its exit code and output. It runs as the user and is not limited to the workspace. It stops after 2 minutes and only the first 20 KB of output is kept. The user is asked before it runs. Prefer the file tools for reading and changing files.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command line, as typed in a terminal."}
                },
                "required": ["command"]
            }),
        ),
        ToolSpec {
            name: super::ask::TOOL,
            family: Family::Ask,
            capability: Capability::Ask,
            description: super::ask::description(),
            parameters: super::ask::parameters(),
        },
        ToolSpec {
            name: super::web::TOOL,
            family: Family::Web,
            capability: Capability::Fetch,
            description: super::web::description(),
            parameters: super::web::parameters(),
        },
    ]
}

/// The permission a spec currently has in a session's configuration. A
/// capability that was never configured has its default.
pub fn permission_of(config: &EffectiveToolConfig, spec: &ToolSpec) -> Permission {
    let wanted = config
        .capabilities
        .get(&capability_key(spec.family, spec.capability))
        .copied()
        .unwrap_or_else(|| default_permission(spec.family, spec.capability));
    // Auto > Ask > Off: a setting above the maximum is lowered to it.
    match (wanted, max_permission(spec.family, spec.capability)) {
        (Permission::Auto, Permission::Ask) => Permission::Ask,
        (Permission::Auto | Permission::Ask, Permission::Off) => Permission::Off,
        (w, _) => w,
    }
}

/// Whether the tool is switched on for the session.
pub fn tool_enabled(config: &EffectiveToolConfig, spec: &ToolSpec) -> bool {
    config.tools.get(spec.name).copied().unwrap_or(true)
}

/// Checked at call time: the tool must be on and its capability allowed.
pub fn check(config: &EffectiveToolConfig, spec: &ToolSpec) -> Result<(), ToolError> {
    if !tool_enabled(config, spec) {
        return Err(ToolError::Off(spec.name.to_string()));
    }
    match permission_of(config, spec) {
        Permission::Off => Err(ToolError::Off(spec.name.to_string())),
        Permission::Ask => Err(ToolError::NeedsApproval(spec.name.to_string())),
        Permission::Auto => Ok(()),
    }
}

/// The tools the model is told about: those that are on and not forbidden.
/// A tool set to Ask is offered; its calls wait for the user.
pub fn offered(config: &EffectiveToolConfig) -> Vec<ToolSpec> {
    specs()
        .into_iter()
        .filter(|s| matches!(check(config, s), Ok(()) | Err(ToolError::NeedsApproval(_))))
        .collect()
}

/// True when a call to `name` has to be approved first: the tool exists, is
/// on, and its capability is set to Ask.
pub fn needs_approval(config: &EffectiveToolConfig, name: &str) -> bool {
    specs()
        .iter()
        .find(|s| s.name == name)
        .is_some_and(|s| matches!(check(config, s), Err(ToolError::NeedsApproval(_))))
}

/// True when a tool works inside the agent's working directory, so a call
/// has to wait until one is attached. Questions and web fetches do not.
pub fn needs_workspace(name: &str) -> bool {
    specs()
        .iter()
        .find(|s| s.name == name)
        .is_some_and(|s| matches!(s.family, Family::Files | Family::Shell))
}

/// The settings key of the capability a tool belongs to (for "allow for this
/// chat").
pub fn capability_of(name: &str) -> Option<String> {
    specs()
        .iter()
        .find(|s| s.name == name)
        .map(|s| capability_key(s.family, s.capability))
}

/// `tools` for an Ollama chat request.
pub fn ollama_definitions(config: &EffectiveToolConfig) -> Value {
    Value::Array(
        offered(config)
            .into_iter()
            .map(|s| {
                json!({
                    "type": "function",
                    "function": {
                        "name": s.name,
                        "description": s.description,
                        "parameters": s.parameters,
                    }
                })
            })
            .collect(),
    )
}

/// Runs one tool call, after checking that it is allowed. A tool set to Ask
/// is refused here; use `call_approved` once the user said yes.
pub fn call(
    ws: &Workspace,
    config: &EffectiveToolConfig,
    name: &str,
    args: &Value,
    limits: &Limits,
) -> Result<String, ToolError> {
    run(ws, config, name, args, limits, &Options::once(false)).0
}

/// Like `call`, for a call the user approved: a tool set to Ask may run. A
/// tool that is off still may not.
pub fn call_approved(
    ws: &Workspace,
    config: &EffectiveToolConfig,
    name: &str,
    args: &Value,
    limits: &Limits,
) -> Result<String, ToolError> {
    run(ws, config, name, args, limits, &Options::once(true)).0
}

/// Like `call` or `call_approved`, for the chat core: also hands back the
/// change a write made, so it can be kept for undo. `undo_room` is how many
/// bytes of originals may still be kept; a write that needs more is refused
/// before anything is touched.
pub fn call_tracked(
    ws: &Workspace,
    config: &EffectiveToolConfig,
    name: &str,
    args: &Value,
    limits: &Limits,
    opts: &Options,
) -> (Result<String, ToolError>, Option<change::Change>) {
    run(ws, config, name, args, limits, opts)
}

/// Whether a call to `name` may run now: the tool exists, is on, and its
/// capability allows it (or asks, and the user said yes).
pub fn authorize(
    config: &EffectiveToolConfig,
    name: &str,
    approved: bool,
) -> Result<(), ToolError> {
    let spec = specs()
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| ToolError::UnknownTool(name.to_string()))?;
    match check(config, &spec) {
        Err(ToolError::NeedsApproval(_)) if approved => Ok(()),
        other => other,
    }
}

fn run(
    ws: &Workspace,
    config: &EffectiveToolConfig,
    name: &str,
    args: &Value,
    limits: &Limits,
    opts: &Options,
) -> (Result<String, ToolError>, Option<change::Change>) {
    let (approved, undo_room, cancel) = (opts.approved, opts.undo_room, opts.cancel);
    let spec = match specs().into_iter().find(|s| s.name == name) {
        Some(s) => s,
        None => return (Err(ToolError::UnknownTool(name.to_string())), None),
    };
    match check(config, &spec) {
        Err(ToolError::NeedsApproval(_)) if approved => {}
        Err(e) => return (Err(e), None),
        Ok(()) => {}
    }
    // A model that sends no arguments means "defaults".
    let empty = json!({});
    let args = if args.is_null() { &empty } else { args };
    if !args.is_object() {
        return (
            Err(ToolError::InvalidArguments(
                "arguments must be an object".into(),
            )),
            None,
        );
    }
    match name {
        "list_dir" => (tools::list_dir(ws, args, limits), None),
        "read_file" => (tools::read_file(ws, args, limits), None),
        "read_binary" => (tools::read_binary(ws, args, limits), None),
        "find_files" => (tools::find_files(ws, args, limits), None),
        "search_text" => (tools::search_text(ws, args, limits), None),
        "search_bytes" => (tools::search_bytes(ws, args, limits), None),
        "write_file" | "edit_file" => write(ws, name, args, undo_room, limits),
        "run_command" => (run_command(ws, args, limits, cancel), None),
        // Fetched by the chat manager, which can wait for the network.
        super::web::TOOL => (
            Err(ToolError::InvalidArguments(
                "this tool is handled by the app, not run here".into(),
            )),
            None,
        ),
        // Asked through the chat manager, which waits for the user.
        super::ask::TOOL => (
            Err(ToolError::InvalidArguments(
                "this tool is handled by the app, not run here".into(),
            )),
            None,
        ),
        other => (Err(ToolError::UnknownTool(other.to_string())), None),
    }
}

/// Runs `command` from the arguments in the workspace folder.
fn run_command(
    ws: &Workspace,
    args: &Value,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<String, ToolError> {
    let Some(Value::String(command)) = args.get("command") else {
        return Err(ToolError::InvalidArguments(
            "command is required and must be a string".into(),
        ));
    };
    super::command::run(ws.root(), command, limits, cancel)
}

/// Plans a write, checks the original can be kept, then applies it.
fn write(
    ws: &Workspace,
    name: &str,
    args: &Value,
    undo_room: usize,
    limits: &Limits,
) -> (Result<String, ToolError>, Option<change::Change>) {
    let change = match change::plan(ws, name, args, limits) {
        Ok(Some(c)) => c,
        Ok(None) => return (Err(ToolError::UnknownTool(name.to_string())), None),
        Err(e) => return (Err(e), None),
    };
    let needed = change.before.as_ref().map_or(0, String::len);
    if needed > undo_room {
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
    use tempfile::TempDir;

    fn cfg() -> EffectiveToolConfig {
        EffectiveToolConfig::default()
    }

    fn names(v: &[ToolSpec]) -> Vec<&'static str> {
        v.iter().map(|s| s.name).collect()
    }

    #[test]
    fn the_tool_definitions_sent_to_the_model_have_not_changed() {
        // Every tool on, every capability automatic.
        let mut config = cfg();
        for s in specs() {
            config
                .capabilities
                .insert(capability_key(s.family, s.capability), Permission::Auto);
        }
        let mut text = serde_json::to_string_pretty(&ollama_definitions(&config)).unwrap();
        text.push('\n');
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/golden/tool_definitions.json"
        );
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
            std::fs::write(path, &text).unwrap();
        }
        let golden = std::fs::read_to_string(path).expect("golden file; run with UPDATE_GOLDEN=1");
        assert_eq!(text, golden);
    }

    #[test]
    fn every_registered_tool_has_a_unique_name_a_description_and_an_object_schema() {
        let tools = super::super::tool::all();
        let found: Vec<&str> = tools.iter().map(|t| t.spec().name).collect();
        for expected in ["list_dir", "read_file"] {
            assert!(found.contains(&expected), "{expected} is not registered");
        }
        let mut seen = std::collections::BTreeSet::new();
        for t in tools {
            let spec = t.spec();
            assert!(seen.insert(spec.name), "{} is registered twice", spec.name);
            assert!(!spec.description.is_empty(), "{} has no description", spec.name);
            assert_eq!(spec.parameters["type"], "object", "{}", spec.name);
        }
    }

    #[test]
    fn the_registry_has_six_read_tools_two_write_tools_a_command_tool_a_web_tool_and_the_question_tool(
    ) {
        let all = specs();
        assert_eq!(
            names(&all),
            [
                "list_dir",
                "read_file",
                "read_binary",
                "find_files",
                "search_text",
                "search_bytes",
                "write_file",
                "edit_file",
                "run_command",
                "ask_user",
                "fetch_url"
            ]
        );
        assert!(all
            .iter()
            .filter(|s| !["run_command", "ask_user", "fetch_url"].contains(&s.name))
            .all(|s| s.family == Family::Files));
        let web = all.iter().find(|s| s.name == "fetch_url").unwrap();
        assert_eq!(
            (web.family, web.capability),
            (Family::Web, Capability::Fetch)
        );
        assert_eq!(
            default_permission(Family::Web, Capability::Fetch),
            Permission::Off,
            "the web is off until the user turns it on"
        );
        assert_eq!(capability_key(Family::Web, Capability::Fetch), "web.fetch");
        let ask = all.iter().find(|s| s.name == "ask_user").unwrap();
        assert_eq!((ask.family, ask.capability), (Family::Ask, Capability::Ask));
        assert_eq!(
            default_permission(Family::Ask, Capability::Ask),
            Permission::Auto,
            "asking needs no permission"
        );
        let run = all.iter().find(|s| s.name == "run_command").unwrap();
        assert_eq!(
            (run.family, run.capability),
            (Family::Shell, Capability::Run)
        );
        let writes: Vec<_> = all
            .iter()
            .filter(|s| s.capability == Capability::Write)
            .map(|s| s.name)
            .collect();
        assert_eq!(writes, ["write_file", "edit_file"]);
    }

    #[test]
    fn a_new_session_reads_automatically_and_asks_before_writing() {
        let c = cfg();
        assert_eq!(permission_of(&c, &specs()[0]), Permission::Auto);
        let write = specs()
            .into_iter()
            .find(|s| s.name == "write_file")
            .unwrap();
        assert_eq!(permission_of(&c, &write), Permission::Ask);
        assert!(needs_approval(&c, "write_file"));
        assert!(needs_approval(&c, "edit_file"));
        assert!(!needs_approval(&c, "read_file"));
        assert_eq!(capability_of("edit_file").as_deref(), Some("files.write"));
        // Both are offered to the model; their calls wait for the user.
        assert_eq!(offered(&c).len(), 9);
    }

    #[test]
    fn a_write_that_would_not_fit_in_the_undo_room_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.txt"), "x".repeat(100)).unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let mut c = cfg();
        c.capabilities
            .insert("files.write".into(), Permission::Auto);
        let args = json!({"path": "big.txt", "content": "y"});
        let (r, change) = call_tracked(
            &ws,
            &c,
            "write_file",
            &args,
            &Limits::default(),
            &Options {
                approved: false,
                undo_room: 50,
                cancel: &AtomicBool::new(false),
            },
        );
        assert!(r.is_err() && change.is_none());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("big.txt")).unwrap(),
            "x".repeat(100)
        );
        let (r, change) = call_tracked(
            &ws,
            &c,
            "write_file",
            &args,
            &Limits::default(),
            &Options {
                approved: false,
                undo_room: 100,
                cancel: &AtomicBool::new(false),
            },
        );
        assert!(r.is_ok());
        assert_eq!(change.unwrap().before.unwrap().len(), 100);
    }

    #[test]
    fn writing_can_be_off_ask_or_allowed() {
        let mut c = cfg();
        c.capabilities.insert("files.write".into(), Permission::Off);
        assert_eq!(
            offered(&c).len(),
            7,
            "a read-only agent is not offered writes"
        );
        c.capabilities
            .insert("files.write".into(), Permission::Auto);
        // Every write can be undone, so it may be allowed outright.
        assert!(!needs_approval(&c, "write_file"));
        assert_eq!(
            max_permission(Family::Files, Capability::Write),
            Permission::Auto
        );
    }

    #[test]
    fn a_write_tool_changes_the_file_only_once_approved() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let c = cfg();
        let args = json!({"path": "n.txt", "content": "hi\n"});
        let lim = Limits::default();
        assert_eq!(
            call(&ws, &c, "write_file", &args, &lim),
            Err(ToolError::NeedsApproval("write_file".into()))
        );
        assert!(!dir.path().join("n.txt").exists());
        let out = call_approved(&ws, &c, "write_file", &args, &lim).unwrap();
        assert_eq!(out, "Created n.txt (+1 -0 lines).");
        let edit = json!({"path": "n.txt", "old_text": "hi", "new_text": "yo"});
        call_approved(&ws, &c, "edit_file", &edit, &lim).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("n.txt")).unwrap(),
            "yo\n"
        );
    }

    #[test]
    fn an_unconfigured_session_allows_read_tools_automatically() {
        assert_eq!(offered(&cfg()).len(), 9);
        assert_eq!(
            capability_key(Family::Files, Capability::Read),
            "files.read"
        );
        assert_eq!(
            max_permission(Family::Files, Capability::Read),
            Permission::Auto
        );
    }

    #[test]
    fn a_single_tool_can_be_switched_off_without_losing_the_others() {
        let mut c = cfg();
        c.tools.insert("search_text".into(), false);
        assert_eq!(
            names(&offered(&c)),
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
        let spec = specs()
            .into_iter()
            .find(|s| s.name == "search_text")
            .unwrap();
        assert_eq!(check(&c, &spec), Err(ToolError::Off("search_text".into())));
    }

    #[test]
    fn a_whole_capability_can_be_turned_off_or_set_to_ask() {
        let mut c = cfg();
        c.capabilities.insert("files.read".into(), Permission::Off);
        assert_eq!(names(&offered(&c)), ["write_file", "edit_file", "ask_user"]);
        c.capabilities.insert("files.read".into(), Permission::Ask);
        assert_eq!(offered(&c).len(), 9, "tools that ask are still offered");
        assert!(needs_approval(&c, "read_file"));
        assert!(!needs_approval(&c, "no_such_tool"));
        let spec = &specs()[0];
        assert_eq!(
            check(&c, spec),
            Err(ToolError::NeedsApproval("list_dir".into()))
        );
        assert_eq!(capability_of("read_file").as_deref(), Some("files.read"));
        c.capabilities.insert("files.read".into(), Permission::Auto);
        assert_eq!(offered(&c).len(), 9);
    }

    #[test]
    fn an_approved_call_may_run_a_tool_that_asks_but_never_one_that_is_off() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hi\n").unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let args = json!({"path": "a.txt"});
        let mut c = cfg();
        c.capabilities.insert("files.read".into(), Permission::Ask);
        assert_eq!(
            call(&ws, &c, "read_file", &args, &Limits::default()),
            Err(ToolError::NeedsApproval("read_file".into()))
        );
        assert_eq!(
            call_approved(&ws, &c, "read_file", &args, &Limits::default()),
            Ok("1| hi".to_string())
        );
        c.capabilities.insert("files.read".into(), Permission::Off);
        assert_eq!(
            call_approved(&ws, &c, "read_file", &args, &Limits::default()),
            Err(ToolError::Off("read_file".into()))
        );
    }

    #[test]
    fn ollama_definitions_describe_only_the_allowed_tools() {
        let mut c = cfg();
        c.tools.insert("find_files".into(), false);
        let defs = ollama_definitions(&c);
        let list = defs.as_array().unwrap();
        assert_eq!(list.len(), 8);
        for d in list {
            assert_eq!(d["type"], "function");
            assert!(d["function"]["name"].is_string());
            assert!(d["function"]["description"].as_str().unwrap().len() > 10);
            assert_eq!(d["function"]["parameters"]["type"], "object");
        }
        let read = list
            .iter()
            .find(|d| d["function"]["name"] == "read_file")
            .unwrap();
        assert_eq!(read["function"]["parameters"]["required"][0], "path");
    }

    #[test]
    fn call_runs_allowed_tools_and_refuses_the_rest_at_call_time() {
        let root = TempDir::new().unwrap();
        std::fs::write(root.path().join("a.txt"), "hello\n").unwrap();
        let ws = Workspace::open(root.path()).unwrap();
        let l = Limits::default();
        let out = call(&ws, &cfg(), "read_file", &json!({"path": "a.txt"}), &l).unwrap();
        assert_eq!(out, "1| hello");
        assert_eq!(
            call(&ws, &cfg(), "list_dir", &Value::Null, &l).unwrap(),
            "a.txt"
        );
        assert!(matches!(
            call(&ws, &cfg(), "delete_file", &json!({}), &l),
            Err(ToolError::UnknownTool(_))
        ));
        assert!(matches!(
            call(&ws, &cfg(), "read_file", &json!("a.txt"), &l),
            Err(ToolError::InvalidArguments(_))
        ));
        // Turned off after the model was told about it: still refused.
        let mut c = cfg();
        c.tools.insert("read_file".into(), false);
        assert!(matches!(
            call(&ws, &c, "read_file", &json!({"path": "a.txt"}), &l),
            Err(ToolError::Off(_))
        ));
    }

    #[test]
    fn a_model_cannot_escape_the_workspace_through_any_tool() {
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "classified").unwrap();
        let root = TempDir::new().unwrap();
        let ws = Workspace::open(root.path()).unwrap();
        let l = Limits::default();
        let hostile = [
            (
                "read_file",
                json!({"path": outside.path().join("secret.txt").to_str().unwrap()}),
            ),
            ("read_file", json!({"path": "../secret.txt"})),
            (
                "list_dir",
                json!({"path": outside.path().to_str().unwrap()}),
            ),
            ("list_dir", json!({"path": "../"})),
            ("find_files", json!({"pattern": "*", "path": ".."})),
            ("search_text", json!({"pattern": "classified", "path": "/"})),
            ("read_file", json!({"path": "a\0b"})),
        ];
        for (tool, args) in hostile {
            let r = call(&ws, &cfg(), tool, &args, &l);
            assert!(r.is_err(), "{tool} {args} was allowed: {r:?}");
        }
    }
}
