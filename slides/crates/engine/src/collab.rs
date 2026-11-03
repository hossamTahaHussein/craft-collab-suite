//! Co-editing mode. While it is on, each edit the person makes is recorded once — the outermost
//! command that changed the presentation — with the place it ran at (the slide by its id and the
//! selection on it) and the [`RunCtx`] it ran under, for the co-editing layer to send to the
//! other replicas, which replay it with [`Session::execute_with`]. Undo and redo go to that
//! layer (selective undo for everyone), and what can't be replayed elsewhere is refused.

use std::cell::Cell;
use std::sync::Arc;

use deckcraft_model::{Presentation, SlideId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{EngineError, Result, Selection, Session};

thread_local! {
    static CLOCK: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The time [`Session::execute_with`] fixed for the edit running now (seconds since the Unix
/// epoch), if any.
pub(crate) fn clock() -> Option<u64> {
    CLOCK.get()
}

fn with_clock<R>(time: u64, f: impl FnOnce() -> R) -> R {
    let outer = CLOCK.get();
    if outer.is_none() {
        CLOCK.set(Some(time));
    }
    let r = f();
    CLOCK.set(outer);
    r
}

/// A local wall-clock time (the author's), for date and time fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// Everything an edit reads from outside the presentation, fixed for one run: who made it and
/// when.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RunCtx {
    pub author: String,
    pub initials: String,
    /// Seconds since the Unix epoch: comment times on every replica.
    pub time: u64,
    /// The author's local time then: inserted date and time fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalTime>,
    /// This person's Suggesting mode.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub track: bool,
    /// Made by the relay's HTTP API through this person's editor: not theirs to undo.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub api: bool,
}

impl craft_collab_core::CollabCtx for RunCtx {
    fn author(&self) -> &str {
        &self.author
    }
    fn time(&self) -> u64 {
        self.time
    }
    fn track(&self) -> bool {
        self.track
    }
    fn api(&self) -> bool {
        self.api
    }
}

/// Where an edit ran: the slide by its id (indexes move when slides come and go) and the
/// selection on it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SlidePlace {
    pub slide: SlideId,
    pub selection: Selection,
}

/// One recorded edit.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    pub id: String,
    pub params: Value,
    pub place: Option<SlidePlace>,
    pub ctx: RunCtx,
    /// The presentation as it was before the edit ran: what its place and its parameters (a
    /// slide index, a text position) referred to.
    pub before: Option<Arc<Presentation>>,
}

/// What the person may do in a co-edited presentation (their join token's role).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CollabRole {
    #[default]
    Edit,
    /// Everything, as suggestions others accept or reject (until those come, nothing).
    Suggest,
    /// Comments only.
    Comment,
    /// Read only.
    View,
}

/// What a commenter may change.
pub const COMMENT_COMMANDS: &[&str] = &["comment.add", "comment.reply", "comment.delete", "comment.resolve"];

#[derive(Debug)]
pub(crate) struct CollabState {
    on: bool,
    recording: bool,
    role: CollabRole,
    /// Read only (Viewing, or offline too long) and whether the person may lift it themselves.
    read_only: (bool, bool),
    /// A recorded command is running: the commands it runs aren't recorded again.
    busy: bool,
    outbox: Vec<Recorded>,
}

impl Default for CollabState {
    fn default() -> Self {
        CollabState { on: false, recording: true, role: CollabRole::Edit, read_only: (false, false), busy: false, outbox: Vec::new() }
    }
}

/// Undo and redo while co-editing: recorded for the co-editing layer, which undoes this
/// person's own most recent edit for everyone, instead of running on this session's history.
pub const COLLAB_UNDO: &[&str] = &["edit.undo", "edit.redo"];

/// Commands refused while co-editing: a co-edited deck can't be swapped for another one, and
/// what compares it with a file on this computer can't be replayed by the other replicas.
pub const COLLAB_DISABLED: &[&str] =
    &["file.new", "file.open", "file.openBytes", "file.adopt", "file.close", "file.revert", "file.templates", "review.compare", "review.endCompare"];

impl Session {
    /// Turn co-editing mode on or off. While on, edits are recorded ([`Session::take_outbox`])
    /// and [`COLLAB_DISABLED`] commands are refused.
    pub fn set_collab(&mut self, on: bool) {
        self.collab.on = on;
        self.collab.outbox.clear();
    }

    pub fn collab(&self) -> bool {
        self.collab.on
    }

    /// Pause recording (while the co-editing layer replays commands it already has).
    pub fn set_recording(&mut self, on: bool) {
        self.collab.recording = on;
    }

    pub fn recording(&self) -> bool {
        self.collab.recording
    }

    /// What the person may do while co-editing; the relay refuses the rest anyway, this says so
    /// before the edit shows.
    pub fn set_collab_role(&mut self, role: CollabRole) {
        self.collab.role = role;
    }

    pub fn collab_role(&self) -> CollabRole {
        self.collab.role
    }

    /// Read only while co-editing (`locked`: the person can't lift it, as when offline too long).
    pub fn set_read_only(&mut self, on: bool, locked: bool) {
        self.collab.read_only = (on, locked);
    }

    pub fn read_only(&self) -> (bool, bool) {
        self.collab.read_only
    }

    /// Whether the person may make edit `id` now, and why not.
    fn may_change(&self, id: &str) -> Result<()> {
        let refuse = |why: &str| Err(EngineError::Disabled(id.into(), why.into()));
        if self.collab.read_only.0 {
            return refuse("This presentation is read only for now");
        }
        match self.collab.role {
            CollabRole::Edit => Ok(()),
            CollabRole::Comment if COMMENT_COMMANDS.contains(&id) => Ok(()),
            CollabRole::Comment => refuse("You can only comment on this presentation"),
            CollabRole::Suggest => refuse("Suggesting isn't available in Craft Slides yet"),
            CollabRole::View => refuse("You can only view this presentation"),
        }
    }

    /// The edits recorded since the last call.
    pub fn take_outbox(&mut self) -> Vec<Recorded> {
        std::mem::take(&mut self.collab.outbox)
    }

    /// The context an edit made now in this session would have.
    pub fn run_ctx(&self) -> RunCtx {
        let local = deckcraft_text::datetime::DateTime::now_local().map(|d| LocalTime {
            year: d.year,
            month: d.month,
            day: d.day,
            hour: d.hour,
            minute: d.minute,
            second: d.second,
        });
        RunCtx { author: self.prefs.author.clone(), initials: String::new(), time: crate::cmd::review::now_secs(), local, track: false, api: false }
    }

    /// Where the active presentation's selection is.
    pub fn collab_place(&self) -> Option<SlidePlace> {
        let d = self.active()?;
        let slide = d.doc.slides.get(d.selection.slide)?.id;
        Some(SlidePlace { slide, selection: d.selection.clone() })
    }

    /// Put the active presentation's selection at `place`, on its slide wherever that is now. A
    /// slide that is gone gives the first one: every replica must run an edit on the same slide.
    pub fn set_collab_place(&mut self, place: &SlidePlace) {
        let Some(d) = self.active_mut() else { return };
        let mut sel = place.selection.clone();
        sel.slide = d.doc.slides.iter().position(|s| s.id == place.slide).unwrap_or(0);
        d.selection = sel;
        crate::clamp_selection(d);
    }

    /// Co-editing: the active presentation's content becomes `doc` (the shared one, or the one
    /// rebuilt from it). A session with no presentation gets one.
    pub fn set_collab_presentation(&mut self, doc: Arc<Presentation>) {
        match self.active_mut() {
            Some(d) => {
                d.doc = doc;
                d.revision += 1;
                crate::clamp_selection(d);
            }
            None => {
                self.add_document(crate::DocState::new((*doc).clone(), None, "Shared presentation".into()));
            }
        }
    }

    /// Run a command under `ctx`, as every replica runs an edit: its author and its clock.
    ///
    /// The edit came from someone else, through the relay: what co-editing refuses is refused
    /// here too, and nothing it runs reaches this computer's files.
    pub fn execute_with(&mut self, ctx: &RunCtx, id: &str, params: &Value) -> Result<Value> {
        if COLLAB_DISABLED.contains(&id) || COLLAB_UNDO.contains(&id) {
            return Err(EngineError::Disabled(id.into(), "this isn't available while co-editing".into()));
        }
        let none = crate::file_access::AutomationRoots::none();
        let result = crate::file_access::confine(Some(&none), || self.run_under(ctx, id, params));
        // A replayed edit opens no dialogs on this replica.
        self.ui_requests.clear();
        result
    }

    /// Run a command with `ctx`'s author and clock.
    fn run_under(&mut self, ctx: &RunCtx, id: &str, params: &Value) -> Result<Value> {
        let author = std::mem::replace(&mut self.prefs.author, ctx.author.clone());
        let busy = std::mem::replace(&mut self.collab.busy, true);
        let local = ctx.local.map(|l| deckcraft_text::datetime::DateTime {
            year: l.year,
            month: l.month,
            day: l.day,
            hour: l.hour,
            minute: l.minute,
            second: l.second,
        });
        let result = with_clock(ctx.time, || match local {
            Some(l) => deckcraft_text::datetime::with_local_now(l, || self.execute(id, params)),
            None => self.execute(id, params),
        });
        self.collab.busy = busy;
        self.prefs.author = author;
        result
    }

    /// Whether `execute` must go through [`Session::execute_collab`].
    pub(crate) fn collab_wraps(&self) -> bool {
        self.collab.on && !self.collab.busy && self.depth == 0
    }

    /// [`Session::execute`] in co-editing mode, for a top-level command: refuses what co-editing
    /// can't do and records the command when it changed the presentation.
    pub(crate) fn execute_collab(&mut self, id: &str, params: &Value) -> Result<Value> {
        let refuse = |why: &str| EngineError::Disabled(id.into(), format!("{why} isn't available while co-editing"));
        if COLLAB_DISABLED.contains(&id) {
            return Err(refuse("This"));
        }
        let person = self.collab.recording;
        if COLLAB_UNDO.contains(&id) && person {
            self.may_change(id)?;
            let place = self.collab_place();
            let ctx = self.run_ctx();
            let before = self.active().map(|d| d.doc.clone());
            self.collab.outbox.push(Recorded { id: id.into(), params: params.clone(), place, ctx, before });
            return Ok(serde_json::json!({"queued": true}));
        }
        // Every copy replays the edit, and they can't read this computer's files.
        if person
            && params.get("path").is_some_and(|p| !p.is_null())
            && !id.starts_with("file.save")
            && !id.starts_with("file.export")
            && id != "file.render"
        {
            return Err(refuse("Reading a file from this computer"));
        }
        if !person {
            self.collab.busy = true;
            let r = self.execute(id, params);
            self.collab.busy = false;
            return r;
        }
        let before = self.active().map(|d| (d.uid, d.doc.clone()));
        let place = self.collab_place();
        let ctx = self.run_ctx();
        let result = self.run_under(&ctx, id, params);
        let changed = match (&before, self.active()) {
            (Some((uid, doc)), Some(d)) => d.uid == *uid && !Arc::ptr_eq(doc, &d.doc) && **doc != *d.doc,
            _ => false,
        };
        if result.is_ok() && changed {
            // What the person may not change is put back: the edit never happened.
            if let Err(e) = self.may_change(id) {
                if let (Some((_, doc)), Some(d)) = (before, self.active_mut()) {
                    d.doc = doc;
                    d.revision += 1;
                    crate::clamp_selection(d);
                }
                return Err(e);
            }
            let before = before.map(|(_, doc)| doc);
            self.collab.outbox.push(Recorded { id: id.into(), params: params.clone(), place, ctx, before });
        }
        result
    }
}
