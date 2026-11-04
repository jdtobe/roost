//! Read-only agent building blocks: the workspace jail, the tools and the
//! registry that decides which tools may run. No UI and no model calls here.

pub mod ask;
pub mod change;
pub mod command;
pub mod registry;
pub mod text_call;
pub mod tool;
pub mod tools;
pub mod web;
pub mod workspace;

pub use workspace::{is_sensitive, Entry, EntryKind, ToolError, Workspace};
