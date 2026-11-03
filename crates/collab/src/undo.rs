//! Selective undo and redo (co-editing spec §6), from the suite's co-editing core: undoing one
//! person's edit for everyone, however many edits came after it.

pub use craft_collab_core::undo::{CHECKPOINT_EVERY, KEEP_CHECKPOINTS, TYPING_GAP_S};
