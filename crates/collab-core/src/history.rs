//! The changes each recent revision made: what a replica needs to move an edit's selection from
//! its base revision to now.

use std::collections::VecDeque;

use crate::doc::CollabDoc;
use crate::proto::HistEntry;
use crate::replica::CollabError;

/// How many revisions of changes a replica keeps for moving late edits' selections; an edit
/// whose base is older needs a resync.
pub const HISTORY_REVS: usize = 2000;

pub(crate) struct History<D: CollabDoc> {
    /// Revisions before this have no recorded changes.
    oldest: u64,
    revs: VecDeque<(u64, D::Change)>,
}

impl<D: CollabDoc> History<D> {
    /// The history a snapshot at `rev` carried.
    pub fn from_entries(rev: u64, entries: Vec<HistEntry<D>>) -> History<D> {
        let entries: Vec<HistEntry<D>> = entries.into_iter().filter(|e| e.rev <= rev).collect();
        let oldest = entries.first().map_or(rev, |e| e.rev.saturating_sub(1));
        let revs = entries.into_iter().map(|e| (e.rev, D::from_wire(e.change))).collect();
        History { oldest, revs }
    }

    pub fn entries(&self) -> Vec<HistEntry<D>> {
        self.revs.iter().map(|(rev, change)| HistEntry { rev: *rev, change: D::to_wire(change) }).collect()
    }

    pub fn push(&mut self, rev: u64, change: D::Change) {
        self.revs.push_back((rev, change));
        while self.revs.len() > HISTORY_REVS {
            if let Some((r, _)) = self.revs.pop_front() {
                self.oldest = r;
            }
        }
    }

    /// Move a selection made at `base` through every change after it.
    pub fn rebase(&self, sel: &D::Sel, base: u64) -> Result<D::Sel, CollabError> {
        if base < self.oldest {
            return Err(CollabError::TooOld { base, oldest: self.oldest });
        }
        let mut out = sel.clone();
        for (_, change) in self.revs.iter().filter(|(r, _)| *r > base) {
            out = D::map(&out, change);
        }
        Ok(out)
    }
}
