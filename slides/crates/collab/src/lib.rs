//! Craft Slides co-editing. Several people edit one presentation: each replica applies its own
//! edits at once and the relay's order for everyone's (the suite's `craft_collab_core`), and
//! people's selections follow the slides and shapes others delete and the text they type
//! (`change`). This crate does no I/O and draws nothing.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod adapter;
pub mod change;

pub use adapter::{SLIDE_INDEX_KEYS, SlideOp, SlideSel, SlidesDoc, decode};

/// One person's copy of a co-edited presentation.
pub type Replica = craft_collab_core::replica::Replica<SlidesDoc>;
