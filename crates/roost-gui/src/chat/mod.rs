//! The Chat view: a history rail and a conversation pane. `state` is the
//! view-model (events folded into plain data); rendering lives in `view`.

mod chrome;
pub mod state;
mod tool_limits;
mod tool_list;
pub mod view;

#[cfg(test)]
mod layout_tests;
#[cfg(test)]
mod leak_tests;

pub use state::ChatState;
