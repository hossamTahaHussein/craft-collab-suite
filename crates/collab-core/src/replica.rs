//! One person's copy of a co-edited document.
//!
//! The replica keeps the *confirmed* document — the relay's history up to [`Replica::rev`],
//! identical on every replica — beside the *visible* one the person edits, which is confirmed
//! plus their *pending* edits (sent or not yet confirmed). Their own edits show at once; the
//! relay's order decides. When someone else's edit arrives first, the visible document is
//! rebuilt: confirmed, then the pending edits re-run with their selections moved through the
//! change.
//!
//! An edit's selection is recorded against the confirmed document at its `base` revision, and
//! a replica has at most one edit in flight: it sends the next only after the relay has
//! confirmed the last. So the edits between an incoming edit's base and now are all someone
//! else's, and every replica moves its selection through exactly those changes ([`History`])
//! before running it.

use std::collections::VecDeque;

use serde_json::Value;

use crate::doc::{CollabDoc, Local};
use crate::history::History;
use crate::proto::{HistEntry, Op, OpKind, Outbound};
use crate::undo::UndoLog;

/// A fingerprint of the confirmed document every this many revisions.
pub const HASH_EVERY: u64 = 50;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CollabError {
    #[error("missed edits: have revision {have}, got {got}")]
    Gap { have: u64, got: u64 },
    #[error("edit based on revision {base}, older than this replica's history (from {oldest})")]
    TooOld { base: u64, oldest: u64 },
    #[error("bad snapshot: {0}")]
    Snapshot(String),
}

/// What [`Replica::receive`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    /// The relay confirmed this replica's own edit; the visible document already had it.
    Echo,
    /// Someone else's edit; `dropped_pending` of this person's unconfirmed edits could no longer
    /// be applied after it and were dropped.
    Remote { dropped_pending: usize },
    /// An edit this replica already has (sent again after a reconnect): nothing to do.
    Duplicate,
}

pub struct Replica<D: CollabDoc> {
    client: String,
    rev: u64,
    confirmed: D::Session,
    /// Replays the old visible chain while rebasing.
    scratch: D::Session,
    pending: VecDeque<Op<D>>,
    /// Whether `pending[0]` has been sent.
    in_flight: bool,
    next_seq: u64,
    history: History<D>,
    /// The last revision whose fingerprint was reported.
    hashed: u64,
    /// The confirmed edits since the link last looked, for following people's positions.
    moved: Vec<Moved<D>>,
    /// The confirmed document was replaced by a snapshot since the link last looked.
    restarted: bool,
    /// Checkpoints and confirmed edits for selective undo.
    undo: UndoLog<D>,
    /// This person's undone steps (each the edits one undo took back), newest last: what Redo
    /// brings back (emptied by a new edit of theirs).
    undone: Vec<Vec<u64>>,
    /// Messages for the person ("Nothing of yours to undo").
    notes: Vec<String>,
    /// An undo or redo named an edit from before this replica's oldest checkpoint: it must start
    /// again from a fresh snapshot.
    needs_fresh: bool,
}

/// One confirmed edit as it moves positions: its revision, who made it, what it changed (in the
/// confirmed document before it) and where it left its author's selection.
#[derive(Clone, Debug, PartialEq)]
pub struct Moved<D: CollabDoc> {
    pub rev: u64,
    pub client: String,
    pub spans: D::Change,
    pub author_sel: Option<D::Sel>,
}

impl<D: CollabDoc> Replica<D> {
    /// Start from the relay's snapshot at `rev` and the edits after it.
    pub fn join(client: String, rev: u64, snapshot: D::Doc, tail: Vec<Op<D>>) -> Result<Replica<D>, CollabError> {
        Self::join_with_history(client, rev, snapshot, Vec::new(), tail)
    }

    /// [`Replica::join`] with the changes of the revisions before the snapshot, so edits in
    /// `tail` based on an earlier revision can be placed.
    pub fn join_with_history(
        client: String,
        rev: u64,
        snapshot: D::Doc,
        history: Vec<HistEntry<D>>,
        tail: Vec<Op<D>>,
    ) -> Result<Replica<D>, CollabError> {
        let undo = UndoLog::new(rev, &snapshot);
        let empty = snapshot.clone();
        let mut replica = Replica {
            client,
            rev,
            undo,
            undone: Vec::new(),
            notes: Vec::new(),
            needs_fresh: false,
            confirmed: D::headless(snapshot),
            scratch: D::headless(empty),
            pending: VecDeque::new(),
            in_flight: false,
            next_seq: 1,
            history: History::from_entries(rev, history),
            hashed: rev,
            moved: Vec::new(),
            restarted: false,
        };
        for op in tail {
            replica.check_next(&op)?;
            replica.apply_confirmed(&op)?;
        }
        Ok(replica)
    }

    /// Show the confirmed document in `visible` (at its start) and switch it to collab mode.
    pub fn attach(&self, visible: &mut D::Session) {
        D::attach(visible, D::doc(&self.confirmed));
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }

    pub fn client(&self) -> &str {
        &self.client
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn confirmed(&self) -> &D::Doc {
        D::doc(&self.confirmed)
    }

    /// The confirmed document's fingerprint, once per [`HASH_EVERY`] revisions.
    pub fn hash_due(&mut self) -> Option<Outbound<D>> {
        let at = self.rev - self.rev % HASH_EVERY;
        if at == 0 || at <= self.hashed || at != self.rev {
            return None;
        }
        self.hashed = at;
        Some(Outbound::Hash { rev: at, h: D::hash(D::doc(&self.confirmed)) })
    }

    /// The confirmed document as a snapshot, for the relay to give new joiners.
    pub fn snapshot(&self) -> Result<Outbound<D>, CollabError> {
        let doc = D::encode(D::doc(&self.confirmed)).map_err(CollabError::Snapshot)?;
        Ok(Outbound::Snapshot { rev: self.rev, doc, history: self.history.entries() })
    }

    /// Turn the edits the person just made in `visible` into pending ops.
    pub fn drain_local(&mut self, visible: &mut D::Session) {
        for Local { cmd, params, sel, ctx } in D::take_local(visible) {
            // Undo and redo name their edit when they are sent, after the edits before them are
            // confirmed (`next_to_send`).
            let (kind, cmd, params) = match cmd.as_str() {
                "edit.undo" => (OpKind::Undo, "collab.undo".to_string(), serde_json::json!({"target": 0})),
                "edit.redo" => (OpKind::Redo, "collab.redo".to_string(), serde_json::json!({"target": 0})),
                _ => {
                    // A new edit of the person's own ends what they can redo (the HTTP API's
                    // through their editor isn't theirs).
                    if !crate::doc::CollabCtx::api(&ctx) {
                        self.undone.clear();
                    }
                    (OpKind::Cmd, cmd, params)
                }
            };
            self.push_pending(kind, cmd, params, sel, ctx);
        }
    }

    /// Restore the kept version at revision `version` for everyone (the relay puts its document
    /// in the edit). It can be undone like any edit.
    pub fn restore(&mut self, version: u64) {
        self.undone.clear();
        self.push_pending(OpKind::Restore, "collab.restore".into(), serde_json::json!({"version": version}), D::Sel::default(), D::Ctx::default());
    }

    fn push_pending(&mut self, kind: OpKind, cmd: String, params: Value, sel: D::Sel, ctx: D::Ctx) {
        self.pending.push_back(Op { client: self.client.clone(), seq: self.next_seq, base: 0, kind, cmd, params, sel, ctx, rev: None, user: None });
        self.next_seq += 1;
    }

    /// Whether this person has an edit of theirs to undo (or an undone one to redo).
    pub fn can_undo(&self) -> bool {
        self.undo.newest_own(&self.client).is_some()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// True once after an undo or redo reached past what this replica holds: ask for a fresh
    /// resync (`ResyncRequest` with `fresh`).
    pub fn take_needs_fresh(&mut self) -> bool {
        std::mem::take(&mut self.needs_fresh)
    }

    /// Messages for the person since the last call.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// The relay refused this replica's edit `seq` (it was never sequenced): it leaves the
    /// visible document and the next edit can go.
    pub fn refused(&mut self, seq: u64, visible: &mut D::Session) {
        self.drain_local(visible);
        let Some(i) = self.pending.iter().position(|p| p.seq == seq) else { return };
        let named = |op: &Op<D>| -> Option<Vec<u64>> {
            let target = op.params.get("target").and_then(Value::as_u64).filter(|t| *t > 0)?;
            let more = op.params.get("more").and_then(Value::as_array).map(|m| m.iter().filter_map(Value::as_u64).collect::<Vec<_>>());
            Some(std::iter::once(target).chain(more.unwrap_or_default()).collect())
        };
        match self.pending[i].kind {
            // An undo that didn't happen leaves nothing to redo.
            OpKind::Undo => {
                if let Some(t) = named(&self.pending[i]).and_then(|g| g.first().copied())
                    && self.undone.last().and_then(|g| g.first()) == Some(&t)
                {
                    self.undone.pop();
                }
            }
            // A redo that didn't happen can be tried again.
            OpKind::Redo => {
                if let Some(group) = named(&self.pending[i]) {
                    self.undone.push(group);
                }
            }
            _ => {}
        }
        if i == 0 {
            self.in_flight = false;
        }
        let before = D::doc(&self.confirmed).clone();
        self.rebuild_skipping(visible, before, None, |op| op.seq == seq);
    }

    /// The next op to send to the relay, if none is in flight: edits that one edit can do
    /// together (plain typing) are merged first.
    pub fn next_to_send(&mut self) -> Option<Op<D>> {
        if self.in_flight {
            return None;
        }
        self.merge_waiting();
        // An undo or redo names its edit once (one sent again after a reconnect keeps it).
        let unnamed = |op: &Op<D>| matches!(op.kind, OpKind::Undo | OpKind::Redo) && op.params.get("target").and_then(Value::as_u64) == Some(0);
        while self.pending.front().is_some_and(unnamed) && !self.name_target() {
            self.pending.pop_front();
        }
        let rev = self.rev;
        let op = self.pending.front_mut()?;
        op.base = rev;
        self.in_flight = true;
        Some(op.clone())
    }

    /// The undo or redo first in line names its edit; false (with a note) when there is none.
    fn name_target(&mut self) -> bool {
        let Some(op) = self.pending.front() else { return false };
        let undo = op.kind == OpKind::Undo;
        let group = if undo {
            Some(self.undo.own_group(&self.client)).filter(|g| !g.is_empty()).inspect(|g| self.undone.push(g.clone()))
        } else {
            self.undone.pop()
        };
        match (group, self.pending.front_mut()) {
            (Some(g), Some(op)) => {
                op.params = if g.len() > 1 { serde_json::json!({"target": g[0], "more": &g[1..]}) } else { serde_json::json!({"target": g[0]}) };
                true
            }
            _ => {
                self.notes.push(if undo { "Nothing of yours to undo".into() } else { "Nothing of yours to redo".into() });
                false
            }
        }
    }

    /// Apply an op from the relay.
    pub fn receive(&mut self, op: Op<D>, visible: &mut D::Session) -> Result<Applied, CollabError> {
        if op.rev.is_some_and(|r| r <= self.rev) {
            return Ok(Applied::Duplicate);
        }
        self.check_next(&op)?;
        // Edits the person made since the host last collected them were made on the visible
        // document as it is now: take them before it is rebuilt.
        self.drain_local(visible);
        self.apply_from_relay(op, visible)
    }

    /// One relay edit: this replica's own (the first pending one) or someone else's.
    fn apply_from_relay(&mut self, op: Op<D>, visible: &mut D::Session) -> Result<Applied, CollabError> {
        let own = op.client == self.client && self.pending.front().is_some_and(|p| p.seq == op.seq);
        if own && op.kind == OpKind::Cmd {
            self.apply_confirmed(&op)?;
            self.pending.pop_front();
            self.in_flight = false;
            return Ok(Applied::Echo);
        }
        let before = D::doc(&self.confirmed).clone();
        let change = self.apply_confirmed(&op)?;
        // An undo, redo or restore of this person's: the visible document never had it.
        if own {
            self.pending.pop_front();
            self.in_flight = false;
        }
        let hint = D::hint_in(&change, &op.sel);
        let dropped_pending = if self.pending.is_empty() { self.follow(visible, &change) } else { self.rebuild(visible, before, hint) };
        Ok(Applied::Remote { dropped_pending })
    }

    /// The confirmed edits since the last call, oldest first.
    pub fn take_moved(&mut self) -> Vec<Moved<D>> {
        std::mem::take(&mut self.moved)
    }

    /// True once after the confirmed document was replaced by a snapshot: positions kept in the
    /// old one's terms must be placed again (see [`Replica::rebase_anchor`]).
    pub fn take_restarted(&mut self) -> bool {
        std::mem::take(&mut self.restarted)
    }

    /// A selection made at confirmed revision `base`, moved through every edit since, in the
    /// confirmed document's terms; `None` when `base` is newer than this replica or older than
    /// its history.
    pub fn rebase_anchor(&self, sel: &D::Sel, base: u64) -> Option<D::Sel> {
        if base > self.rev {
            return None;
        }
        self.history.rebase(sel, base).ok()
    }

    /// Skip revision `rev`, which no replica could read (a malformed edit): every replica skips
    /// it the same way, as an edit that changed nothing.
    pub fn skip(&mut self, rev: u64) -> Result<(), CollabError> {
        if rev != self.rev + 1 {
            return Err(CollabError::Gap { have: self.rev, got: rev });
        }
        self.rev = rev;
        self.history.push(rev, D::Change::default());
        self.moved.push(Moved { rev, client: String::new(), spans: D::Change::default(), author_sel: None });
        Ok(())
    }

    /// After a reconnect (`Welcome`) or a `Resync`: start again from `snapshot` at `rev` if
    /// given — `seen` is the last of this replica's edits the snapshot holds, which aren't sent
    /// again — then apply `tail` one edit at a time, exactly as if it had arrived live. Returns
    /// the next edit to send (the rest follow one at a time).
    pub fn resume(
        &mut self,
        rev: u64,
        snapshot: Option<(D::Doc, Vec<HistEntry<D>>)>,
        tail: Vec<Op<D>>,
        seen: Option<u64>,
        visible: &mut D::Session,
    ) -> Result<Vec<Op<D>>, CollabError> {
        self.drain_local(visible);
        self.in_flight = false;
        if let Some((doc, history)) = snapshot {
            let before = D::doc(&std::mem::replace(&mut self.confirmed, D::headless(doc))).clone();
            self.undo.restart(rev, D::doc(&self.confirmed));
            self.rev = rev;
            self.history = History::from_entries(rev, history);
            self.hashed = rev;
            self.moved.clear();
            self.restarted = true;
            let seen = seen.unwrap_or(0);
            self.rebuild_skipping(visible, before, None, |op| op.seq <= seen);
        }
        for op in tail {
            if op.rev.is_some_and(|r| r <= self.rev) {
                continue;
            }
            self.check_next(&op)?;
            self.apply_from_relay(op, visible)?;
        }
        Ok(self.next_to_send().into_iter().collect())
    }

    fn check_next(&self, op: &Op<D>) -> Result<(), CollabError> {
        let got = op.rev.unwrap_or(0);
        if got != self.rev + 1 {
            return Err(CollabError::Gap { have: self.rev, got });
        }
        Ok(())
    }

    /// Run a relay op on the confirmed document and record what it changed.
    fn apply_confirmed(&mut self, op: &Op<D>) -> Result<D::Change, CollabError> {
        let sel = self.history.rebase(&op.sel, op.base)?;
        let before = D::doc(&self.confirmed).clone();
        match op.kind {
            // A command that fails here fails on every replica: it is skipped everywhere.
            OpKind::Cmd => {
                let _ = D::run_at(&mut self.confirmed, op, &sel);
            }
            OpKind::Restore => D::restore_into(&mut self.confirmed, op),
            // An edit out of this replica's reach changes nothing here; the fingerprint check
            // resyncs a replica that ends up different.
            OpKind::Undo | OpKind::Redo => {
                let mut targets: Vec<u64> = op.params.get("target").and_then(Value::as_u64).into_iter().collect();
                if let Some(more) = op.params.get("more").and_then(Value::as_array) {
                    targets.extend(more.iter().filter_map(Value::as_u64));
                }
                if targets.iter().any(|t| *t > 0 && *t <= self.undo.oldest()) {
                    self.needs_fresh = true;
                }
                if let Some(doc) = self.undo.apply(&targets, op.kind == OpKind::Undo) {
                    D::set_doc(&mut self.confirmed, doc);
                    D::clamp(&mut self.confirmed);
                }
            }
        }
        let change = D::changes(&before, D::doc(&self.confirmed), D::hint_at(&sel));
        self.rev += 1;
        let mut logged = op.clone();
        logged.sel = sel;
        logged.base = self.rev - 1;
        self.undo.record(self.rev, logged, D::doc(&self.confirmed));
        self.history.push(self.rev, change.clone());
        let author_sel = D::selection(&self.confirmed);
        self.moved.push(Moved { rev: self.rev, client: op.client.clone(), spans: change.clone(), author_sel });
        Ok(change)
    }

    /// Nothing pending: the visible document becomes the confirmed one, and the person's
    /// selection moves through the change.
    fn follow(&mut self, visible: &mut D::Session, change: &D::Change) -> usize {
        let sel = D::selection(visible).map(|a| D::map(&a, change));
        D::set_doc(visible, D::doc(&self.confirmed).clone());
        if let Some(a) = sel {
            D::select(visible, &a);
        }
        D::settle(visible);
        0
    }

    /// Rebuild the visible document as confirmed + pending, each pending edit's selection moved
    /// from the old chain (`before` + pending) to the new one. Returns how many pending edits
    /// failed and were dropped (an edit in flight is kept: the relay will confirm it).
    fn rebuild(&mut self, visible: &mut D::Session, before: D::Doc, hint: Option<D::Hint>) -> usize {
        self.rebuild_skipping(visible, before, hint, |_| false)
    }

    /// [`Self::rebuild`], where pending edits matching `confirmed_already` are part of the old
    /// chain but already in the confirmed document: they are replayed on the old chain only and
    /// leave the pending list.
    ///
    /// `hint`: where the remote change happened, in `before`'s terms — it decides between
    /// places a change of repeated content could be, and follows the old chain's own edits.
    fn rebuild_skipping(
        &mut self,
        visible: &mut D::Session,
        before: D::Doc,
        mut hint: Option<D::Hint>,
        confirmed_already: impl Fn(&Op<D>) -> bool,
    ) -> usize {
        let old_visible = D::doc(visible).clone();
        let caret = D::selection(visible);
        D::set_doc(&mut self.scratch, before);
        D::set_doc(visible, D::doc(&self.confirmed).clone());
        let saved = D::begin_replay(visible);
        let mut kept = VecDeque::new();
        let mut dropped = 0;
        for (i, mut op) in std::mem::take(&mut self.pending).into_iter().enumerate() {
            if op.kind != OpKind::Cmd {
                // Undo, redo and restore take effect when the relay confirms them.
                if !confirmed_already(&op) {
                    kept.push_back(op);
                }
                continue;
            }
            let moved = D::step(D::doc(&self.scratch), D::doc(visible), &op.sel, hint);
            let new_sel = D::move_sel(&op.sel, moved.as_ref());
            let old_chain = D::doc(&self.scratch).clone();
            let _ = D::run_at(&mut self.scratch, &op, &op.sel.clone());
            // The remote change's place moves with this edit on the old chain.
            if hint.is_some()
                && let Some(own) = D::step(&old_chain, D::doc(&self.scratch), &op.sel, None)
            {
                hint = D::move_hint(hint, &op.sel, &own);
            }
            if confirmed_already(&op) {
                continue;
            }
            let ok = D::run_at(visible, &op, &new_sel);
            op.sel = new_sel;
            if ok || (i == 0 && self.in_flight) {
                kept.push_back(op);
            } else {
                dropped += 1;
            }
        }
        D::end_replay(visible, saved);
        self.pending = kept;
        if let Some(a) = caret {
            let moved = D::step(&old_visible, D::doc(visible), &a, hint);
            D::select(visible, &D::move_sel(&a, moved.as_ref()));
        }
        D::settle(visible);
        dropped
    }

    /// Merge edits waiting behind the first pending op into it (not the op in flight), while
    /// one edit of both gives the same document ([`CollabDoc::merge`]).
    fn merge_waiting(&mut self) {
        while self.pending.len() >= 2 {
            let Some(next) = self.pending.get(1).cloned() else { return };
            let Some(first) = self.pending.front_mut() else { return };
            if !D::merge(first, &next) {
                return;
            }
            self.pending.remove(1);
        }
    }
}
