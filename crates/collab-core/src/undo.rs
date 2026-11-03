//! Selective undo and redo: undoing one person's edit for everyone, however many edits came
//! after it.
//!
//! Every replica keeps checkpoints of the confirmed document every [`CHECKPOINT_EVERY`] revisions
//! (the last [`KEEP_CHECKPOINTS`]) and the confirmed edits since the oldest. Undoing edit R
//! rebuilds the confirmed document from the newest checkpoint before R: the edits after it are
//! replayed twice, once as they were (the old chain) and once without R (the new chain), and each
//! replayed edit's selection moves through the difference between its two starting documents, so
//! later edits land where they belong. The relay's order is the same everywhere, so is the result.

use std::collections::{BTreeSet, VecDeque};

use crate::doc::{CollabCtx, CollabDoc};
use crate::proto::{Op, OpKind};

/// A checkpoint every this many revisions.
pub const CHECKPOINT_EVERY: u64 = 100;
/// Checkpoints kept: undo reaches at least `(KEEP_CHECKPOINTS - 1) * CHECKPOINT_EVERY` edits back.
pub const KEEP_CHECKPOINTS: usize = 5;

/// Typing with pauses up to this long (seconds) is one undo step.
pub const TYPING_GAP_S: u64 = 1;

/// One confirmed edit, its selection as it applies in the current chain.
#[derive(Clone, Debug)]
pub(crate) struct Logged<D: CollabDoc> {
    pub rev: u64,
    pub op: Op<D>,
}

pub(crate) struct UndoLog<D: CollabDoc> {
    /// The confirmed document after each checkpoint revision, oldest first.
    checkpoints: VecDeque<(u64, D::Doc)>,
    /// The confirmed edits after the oldest checkpoint.
    entries: VecDeque<Logged<D>>,
    /// The edits undone (and not redone), by revision.
    excluded: BTreeSet<u64>,
}

/// An edit an undo can take back: a command or a restore by `client` themselves (not one the
/// relay's HTTP API made through their editor).
fn undoable<D: CollabDoc>(op: &Op<D>, client: &str) -> bool {
    op.client == client && !op.ctx.api() && matches!(op.kind, OpKind::Cmd | OpKind::Restore)
}

impl<D: CollabDoc> UndoLog<D> {
    /// Start at the confirmed document `doc` at `rev`.
    pub fn new(rev: u64, doc: &D::Doc) -> UndoLog<D> {
        UndoLog { checkpoints: VecDeque::from([(rev, doc.clone())]), entries: VecDeque::new(), excluded: BTreeSet::new() }
    }

    /// Start again from a new document (after a resync). What was undone is forgotten too: the
    /// snapshot already holds the undos up to `rev`, and the edits replayed after it (undos
    /// included) set the rest again.
    pub fn restart(&mut self, rev: u64, doc: &D::Doc) {
        self.checkpoints = VecDeque::from([(rev, doc.clone())]);
        self.entries.clear();
        self.excluded.clear();
    }

    /// The oldest revision an undo can reach is the one after this.
    pub fn oldest(&self) -> u64 {
        self.checkpoints.front().map_or(0, |(r, _)| *r)
    }

    /// Edit `op` (its selection as applied) became revision `rev`, leaving `after`.
    pub fn record(&mut self, rev: u64, op: Op<D>, after: &D::Doc) {
        self.entries.push_back(Logged { rev, op });
        if rev.is_multiple_of(CHECKPOINT_EVERY) {
            self.checkpoints.push_back((rev, after.clone()));
        }
        while self.checkpoints.len() > KEEP_CHECKPOINTS {
            self.checkpoints.pop_front();
        }
        let oldest = self.oldest();
        while self.entries.front().is_some_and(|e| e.rev <= oldest) {
            self.entries.pop_front();
        }
    }

    /// The newest edit by `client` an undo can target: an edit or restore, not undone, after the
    /// oldest checkpoint.
    pub fn newest_own(&self, client: &str) -> Option<u64> {
        self.entries.iter().rev().find(|e| undoable(&e.op, client) && !self.excluded.contains(&e.rev)).map(|e| e.rev)
    }

    /// What one undo by `client` takes back, newest first: their newest edit and, when it is
    /// typing, the run of typing before it (their typing edits each within [`TYPING_GAP_S`] of
    /// the next, with no other edit of theirs between), as Word undoes a typed run at once.
    pub fn own_group(&self, client: &str) -> Vec<u64> {
        let own: Vec<&Logged<D>> = self.entries.iter().rev().filter(|e| undoable(&e.op, client) && !self.excluded.contains(&e.rev)).collect();
        let Some(first) = own.first() else { return Vec::new() };
        let mut group = vec![first.rev];
        let typing = |e: &Logged<D>| e.op.kind == OpKind::Cmd && D::is_typing(&e.op);
        if typing(first) {
            for pair in own.windows(2) {
                let (later, earlier) = (pair[0], pair[1]);
                if !typing(earlier) || later.op.ctx.time().saturating_sub(earlier.op.ctx.time()) > TYPING_GAP_S {
                    break;
                }
                group.push(earlier.rev);
            }
        }
        group
    }

    /// Whether `rev` is an edit an undo or redo can reach.
    fn reachable(&self, rev: u64) -> bool {
        rev > self.oldest() && self.entries.iter().any(|e| e.rev == rev && matches!(e.op.kind, OpKind::Cmd | OpKind::Restore))
    }

    /// Undo (`undo: true`) or redo the edits `targets`: returns the rebuilt confirmed document,
    /// or `None` when nothing changes (out of reach, or already in that state).
    pub fn apply(&mut self, targets: &[u64], undo: bool) -> Option<D::Doc> {
        let targets: Vec<u64> = targets.iter().copied().filter(|t| self.reachable(*t) && self.excluded.contains(t) != undo).collect();
        let first = *targets.iter().min()?;
        let old_excluded = self.excluded.clone();
        let changed: BTreeSet<u64> = targets.iter().copied().collect();
        for target in targets {
            if undo {
                self.excluded.insert(target);
            } else {
                self.excluded.remove(&target);
            }
        }
        let ci = self.checkpoints.iter().rposition(|(r, _)| *r < first)?;
        let start = self.checkpoints[ci].0;
        let mut old = D::headless(self.checkpoints[ci].1.clone());
        let mut new = D::headless(self.checkpoints[ci].1.clone());
        for entry in self.entries.iter_mut().filter(|e| e.rev > start) {
            let moved = D::step(D::doc(&old), D::doc(&new), &entry.op.sel, None);
            let mut new_sel = D::move_sel(&entry.op.sel, moved.as_ref());
            // Redoing a run of typing: each piece goes after the one redone before it, not at
            // the same place (a position at an insertion stays before it otherwise).
            if !undo
                && changed.contains(&entry.rev)
                && let Some(step) = moved.as_ref()
                && let Some(after) = D::after_insertion(&new_sel, step)
            {
                new_sel = after;
            }
            if !old_excluded.contains(&entry.rev) {
                apply_logged::<D>(&mut old, &entry.op, &entry.op.sel.clone());
            }
            entry.op.sel = new_sel.clone();
            if !self.excluded.contains(&entry.rev) {
                apply_logged::<D>(&mut new, &entry.op, &new_sel);
            }
            // Later checkpoints described the old chain.
            if let Some(cp) = self.checkpoints.iter_mut().find(|(r, _)| *r == entry.rev) {
                cp.1 = D::doc(&new).clone();
            }
        }
        Some(D::doc(&new).clone())
    }
}

/// Replay a logged edit: a command runs at its selection, a restore puts its document back;
/// undo and redo themselves change nothing here (the excluded set already says what they did).
fn apply_logged<D: CollabDoc>(s: &mut D::Session, op: &Op<D>, sel: &D::Sel) {
    match op.kind {
        OpKind::Cmd => {
            let _ = D::run_at(s, op, sel);
        }
        OpKind::Restore => D::restore_into(s, op),
        OpKind::Undo | OpKind::Redo => {}
    }
}
