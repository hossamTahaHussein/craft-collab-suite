//! One person's copy of a co-edited document (spec §4): the suite's co-editing replica
//! (`craft_collab_core::replica`) on a WordCraft document.

pub use craft_collab_core::replica::{Applied, CollabError};

use crate::adapter::DocsDoc;

/// One person's copy of a co-edited document.
pub type Replica = craft_collab_core::replica::Replica<DocsDoc>;
/// One confirmed edit as it moves positions: its revision, who made it, what it changed (per
/// story) and where it left its author's selection.
pub type Moved = craft_collab_core::replica::Moved<DocsDoc>;
