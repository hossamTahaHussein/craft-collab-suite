//! What an editor provides to be co-edited: its document, its session (the document plus a
//! selection, run by commands) and how positions move through an edit.
//!
//! The replica ([`crate::replica`]) only ever runs commands, compares documents and moves
//! selections through what changed; everything app-specific — what a position is (a story
//! offset in Docs, a cell in Sheets, a shape's text in Slides), how a change is found and how
//! a selection follows it — lives behind this trait.

use std::fmt::Debug;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::proto::Op;

/// The context an edit ran under, as far as the replica needs it.
pub trait CollabCtx: Clone + Debug + Default + PartialEq + Serialize + DeserializeOwned {
    /// Who made it (the person's name).
    fn author(&self) -> &str;
    /// When (seconds since the Unix epoch): a run of typing is one undo step by it.
    fn time(&self) -> u64;
    /// Made in this person's Suggesting mode.
    fn track(&self) -> bool;
    /// Made by the relay's HTTP API through this person's editor: not theirs to undo.
    fn api(&self) -> bool;
}

/// One edit the person made in the visible document, waiting to become an op.
#[derive(Clone, Debug)]
pub struct Local<D: CollabDoc> {
    pub cmd: String,
    pub params: Value,
    /// The selection it ran at (the default when there was none to record).
    pub sel: D::Sel,
    pub ctx: D::Ctx,
}

/// An editor that can be co-edited. Implemented by a marker type per app.
pub trait CollabDoc: Clone + Debug + Default + PartialEq + 'static {
    type Doc: Clone;
    type Session;
    /// A selection in the confirmed document's terms (offsets, cells, ids): what travels.
    type Sel: Clone + Debug + Default + PartialEq + Serialize + DeserializeOwned;
    type Ctx: CollabCtx;
    /// Everything one revision changed.
    type Change: Clone + Debug + Default + PartialEq;
    /// What one revision changed within the scope of one selection (a story, a sheet, a text
    /// body): what moves that selection.
    type Step: Clone + Debug;
    /// A revision's change as it travels in snapshots (flattened into `HistEntry`).
    type Wire: Clone + Debug + PartialEq + Serialize + DeserializeOwned;
    /// Where a change happened: it decides between equally good comparisons.
    type Hint: Copy + Debug + PartialEq;

    /// A session that runs commands without recording them (the confirmed document's).
    fn headless(doc: Self::Doc) -> Self::Session;
    fn doc(s: &Self::Session) -> &Self::Doc;
    fn set_doc(s: &mut Self::Session, doc: Self::Doc);
    /// Show `doc` in the person's session at its start, in co-editing mode.
    fn attach(visible: &mut Self::Session, doc: &Self::Doc);
    /// The edits the person made since the last call.
    fn take_local(visible: &mut Self::Session) -> Vec<Local<Self>>;
    /// The session's selection, when it is one a [`CollabDoc::Sel`] can hold.
    fn selection(s: &Self::Session) -> Option<Self::Sel>;
    fn select(s: &mut Self::Session, sel: &Self::Sel);
    /// Keep the selection inside the document after it changed under it.
    fn clamp(s: &mut Self::Session);
    /// After the replica replaced or rebuilt the visible document: clamp, and redraw.
    fn settle(visible: &mut Self::Session);
    /// Replays the person's edits on the visible document without recording them again, and
    /// even in Viewing mode; returns what [`CollabDoc::end_replay`] puts back.
    fn begin_replay(visible: &mut Self::Session) -> bool;
    fn end_replay(visible: &mut Self::Session, saved: bool);
    /// Run a command edit at `sel` under its context; false when it failed (it then changes
    /// nothing, on every replica).
    fn run_at(s: &mut Self::Session, op: &Op<Self>, sel: &Self::Sel) -> bool;
    /// Put back the document a restore edit carries; one that can't be read changes nothing.
    fn restore_into(s: &mut Self::Session, op: &Op<Self>);

    /// What changed from `before` to `after`; `hint` says where an edit at the op's selection
    /// happened ([`CollabDoc::hint_at`]).
    fn changes(before: &Self::Doc, after: &Self::Doc, hint: Option<Self::Hint>) -> Self::Change;
    /// Where an edit at `sel` happens.
    fn hint_at(sel: &Self::Sel) -> Option<Self::Hint>;
    /// Where `change` happened within `sel`'s scope.
    fn hint_in(change: &Self::Change, sel: &Self::Sel) -> Option<Self::Hint>;
    /// Move someone else's selection through `change` (another person's edit).
    fn map(sel: &Self::Sel, change: &Self::Change) -> Self::Sel;
    /// What changed within `sel`'s scope from `before` to `after`.
    fn step(before: &Self::Doc, after: &Self::Doc, sel: &Self::Sel, hint: Option<Self::Hint>) -> Option<Self::Step>;
    /// Move `sel` through a step of its own scope.
    fn move_sel(sel: &Self::Sel, step: Option<&Self::Step>) -> Self::Sel;
    /// `hint` (in `sel`'s scope or not) after the edit made at `sel` changed its scope by `own`.
    fn move_hint(hint: Option<Self::Hint>, sel: &Self::Sel, own: &Self::Step) -> Option<Self::Hint>;
    /// Redoing a run of typing: a selection at the place `step` inserted goes after the
    /// insertion (each piece after the one redone before it); `None` leaves it.
    fn after_insertion(sel: &Self::Sel, step: &Self::Step) -> Option<Self::Sel>;

    fn to_wire(change: &Self::Change) -> Self::Wire;
    fn from_wire(wire: Self::Wire) -> Self::Change;
    /// The document's fingerprint: equal on every replica that applied the same edits.
    fn hash(doc: &Self::Doc) -> String;
    /// The document as a snapshot for the relay.
    fn encode(doc: &Self::Doc) -> Result<String, String>;

    /// Whether `op` is a piece of typing (consecutive ones are one undo step).
    fn is_typing(op: &Op<Self>) -> bool;
    /// Merge `next` (waiting behind `first`, which hasn't been sent) into `first` when one edit
    /// of both gives the same document; true when merged.
    fn merge(first: &mut Op<Self>, next: &Op<Self>) -> bool;
}
