//! Co-editing for the suite's editors (Docs, Sheets, Slides). Several people edit one document:
//! each replica applies its own edits at once and the relay's order for everyone's, re-running
//! its unconfirmed edits on top (`replica`). An app plugs in by implementing [`CollabDoc`]: what
//! its document, session and selections are, and how a selection moves through an edit.
//! This crate does no I/O and draws nothing: the host carries `proto` messages to the relay.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod doc;
mod history;
pub mod proto;
pub mod replica;
pub mod undo;

pub use doc::{CollabCtx, CollabDoc, Local};
pub use history::HISTORY_REVS;

#[cfg(test)]
mod line_doc;
#[cfg(test)]
mod tests;
