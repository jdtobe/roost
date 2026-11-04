//! The tools that read files and folders in the workspace.

use super::{OptionDecl, Plan, PlanCx, RunCx, Tool};
use crate::agent::change;
use crate::agent::registry::{Capability, Family, ToolSpec};
use crate::agent::tools;
use crate::agent::workspace::ToolError;
use serde_json::{json, Value};

pub struct ListDir;

impl Tool for ListDir {
    fn name(&self) -> &'static str {
        "list_dir"
    }

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
    fn name(&self) -> &'static str {
        "read_file"
    }

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

pub struct ReadBinary;

impl Tool for ReadBinary {
    fn name(&self) -> &'static str {
        "read_binary"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_binary",
            family: Family::Files,
            capability: Capability::Read,
            description: "Show the bytes of any file (a ROM, an image, an archive …) as a hex dump: offset, 16 bytes in hex, and the same bytes as text. Returns a limited number of bytes per call and says which offset to use to continue. Offsets may be written as 4096 or 0x1000. Use this when read_file says a file is binary.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File relative to the workspace root."},
                    "offset": {"type": "integer", "description": "First byte to show, counting from 0. Defaults to 0."},
                    "length": {"type": "integer", "description": "How many bytes to show (default 256, at most 4096)."}
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
        (tools::read_binary(cx.workspace, args, cx.limits), None)
    }
}

pub struct FindFiles;

impl Tool for FindFiles {
    fn name(&self) -> &'static str {
        "find_files"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "find_files",
            family: Family::Files,
            capability: Capability::Read,
            description: "Find files and directories whose path matches a glob such as *.rs or src/*.toml. A pattern without a slash matches at any depth. Results can be sorted by size or time (to find the largest or newest files). Many results are cut, and the result says which offset to use to see the next ones.",
            parameters: json!({
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
        (tools::find_files(cx.workspace, args, cx.limits), None)
    }
}

pub struct SearchText;

impl Tool for SearchText {
    fn name(&self) -> &'static str {
        "search_text"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search_text",
            family: Family::Files,
            capability: Capability::Read,
            description: "Search text files for a regular expression. Returns path:line: text for each match. Many matches are cut, and the result says which offset to use to see the next ones and which files could not be searched (binary files, files too large).",
            parameters: json!({
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
        (tools::search_text(cx.workspace, args, cx.limits), None)
    }
}

pub struct SearchBytes;

impl Tool for SearchBytes {
    fn name(&self) -> &'static str {
        "search_bytes"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search_bytes",
            family: Family::Files,
            capability: Capability::Read,
            description: "Search files for a byte pattern, such as a ROM header or a file signature. The pattern is hex pairs, with ?? for any byte: \"4E 45 53 1A\" or \"de ?? be ef\". Returns path:0xOFFSET: bytes for each match. Use read_binary to look around an offset. Many matches are cut, and the result says which offset to use to see the next ones.",
            parameters: json!({
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
        (tools::search_bytes(cx.workspace, args, cx.limits), None)
    }
}
