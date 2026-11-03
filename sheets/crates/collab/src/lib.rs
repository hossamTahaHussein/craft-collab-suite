//! Craft Sheets co-editing. Several people edit one workbook: each replica applies its own
//! edits at once and the relay's order for everyone's (the suite's `craft_collab_core`), and
//! people's selections follow the rows, columns and sheets others insert, delete and rename
//! (`mapping`). This crate does no I/O and draws nothing.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod adapter;
pub mod link;
pub mod mapping;
pub mod wire;

pub use adapter::{ADDRESS_KEYS, SheetOp, SheetSel, SheetsDoc, decode};

/// One person's copy of a co-edited workbook.
pub type Replica = craft_collab_core::replica::Replica<SheetsDoc>;
