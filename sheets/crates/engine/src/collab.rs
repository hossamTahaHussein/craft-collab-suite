//! Co-editing mode. While it is on, each edit the person makes is recorded once — the outermost
//! command that changed the workbook — with the place it ran at (sheet and selection) and the
//! [`RunCtx`] it ran under, for the co-editing layer to send to the other replicas, which replay
//! it with [`Session::execute_with`]. Undo and redo go to that layer (selective undo for
//! everyone), and what can't be replayed elsewhere is refused.

use serde::{Deserialize, Serialize};

use crate::{EngineError, Json, Result, Selection, Session};

/// Everything an edit reads from outside the workbook, fixed for one run: who made it and when.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RunCtx {
    pub author: String,
    pub initials: String,
    /// Milliseconds since the Unix epoch: NOW() and TODAY() on every replica, and RAND's seed.
    pub time_ms: u64,
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
        self.time_ms / 1000
    }
    fn track(&self) -> bool {
        self.track
    }
    fn api(&self) -> bool {
        self.api
    }
}

/// Where an edit ran: the sheet (by name) and the selection on it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SheetPlace {
    pub sheet: String,
    pub selection: Selection,
}

/// One recorded edit.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    pub id: String,
    pub params: Json,
    pub place: Option<SheetPlace>,
    pub ctx: RunCtx,
}

/// What the person may do in a co-edited workbook (their join token's role).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CollabRole {
    #[default]
    Edit,
    /// Changes to cells, as suggestions others accept or reject, and comments.
    Suggest,
    /// Comments and notes only.
    Comment,
    /// Read only.
    View,
}

/// What a commenter may change.
pub const COMMENT_COMMANDS: &[&str] =
    &["review.newComment", "review.replyComment", "review.deleteComment", "review.resolveComment", "review.newNote"];

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
    /// The relay's HTTP API is running a command through this editor as this author.
    api_author: Option<String>,
}

impl Default for CollabState {
    fn default() -> Self {
        CollabState {
            on: false,
            recording: true,
            role: CollabRole::Edit,
            read_only: (false, false),
            busy: false,
            outbox: Vec::new(),
            api_author: None,
        }
    }
}

/// Undo and redo while co-editing: recorded for the co-editing layer, which undoes this
/// person's own most recent edit for everyone, instead of running on this session's history.
pub const COLLAB_UNDO: &[&str] = &["edit.undo", "edit.redo"];

/// Commands refused while co-editing: a co-edited workbook can't be swapped for another one, and
/// what runs code or reads this computer's data can't be replayed by the other replicas.
pub const COLLAB_DISABLED: &[&str] = &["file.new", "file.open", "file.close", "file.newFromTemplate", "edit.undoTo", "edit.redoTo"];

/// Families refused while co-editing (macros, scripts, Python, external data).
const COLLAB_DISABLED_PREFIXES: &[&str] = &["macro.", "automate.", "python.", "data.refresh", "data.connections", "data.getData"];

pub(crate) fn disabled(id: &str) -> bool {
    COLLAB_DISABLED.contains(&id) || COLLAB_DISABLED_PREFIXES.iter().any(|p| id.starts_with(p))
}

/// A seed for an edit's random numbers, the same wherever it is replayed.
fn seed(ctx: &RunCtx) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in ctx.author.bytes().chain(ctx.time_ms.to_le_bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

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
        let refuse = |why: &str| Err(EngineError::Disabled(id.to_string(), why.into()));
        if self.collab.read_only.0 {
            return refuse("This workbook is read only for now");
        }
        match self.collab.role {
            CollabRole::Edit => Ok(()),
            CollabRole::Comment if COMMENT_COMMANDS.contains(&id) => Ok(()),
            CollabRole::Comment => refuse("You can only comment on this workbook"),
            CollabRole::Suggest if crate::suggest::SUGGEST_COMMANDS.contains(&id) || COMMENT_COMMANDS.contains(&id) => Ok(()),
            CollabRole::Suggest => refuse("You can only suggest changes to this workbook"),
            CollabRole::View => refuse("You can only view this workbook"),
        }
    }

    /// Whether edits are recorded now (see [`Session::set_recording`]).
    pub fn recording(&self) -> bool {
        self.collab.recording
    }

    /// Co-editing: the active workbook's content becomes `wb` (the shared document, or the one
    /// rebuilt from it), in the same window; its calculation graph follows. A session with no
    /// workbook gets one, untrusted like anything read from elsewhere.
    pub fn set_collab_workbook(&mut self, wb: std::sync::Arc<gridcraft_model::Workbook>) {
        match self.active_mut() {
            // Shared content is never this computer's own, whatever the workbook it replaces was.
            Some(d) => {
                d.trust = crate::Trust::FromFile;
                d.scripts_trusted = false;
                d.calc.rebuild(&wb);
                d.wb = wb;
            }
            None => {
                let d = crate::DocState::from_saved((*wb).clone(), None, "Shared workbook".into(), crate::Trust::FromFile);
                self.add_document(d);
            }
        }
    }

    /// The edits recorded since the last call.
    pub fn take_outbox(&mut self) -> Vec<Recorded> {
        std::mem::take(&mut self.collab.outbox)
    }

    /// The context an edit made now in this session would have.
    pub fn run_ctx(&self) -> RunCtx {
        let time_ms = gridcraft_core::date::unix_now_ms().map_or(0, |ms| ms.max(0.0) as u64);
        match &self.collab.api_author {
            Some(author) => RunCtx { author: author.clone(), initials: initials_of(author), time_ms, track: false, api: true },
            None => RunCtx {
                author: self.prefs.user_name.clone(),
                initials: String::new(),
                time_ms,
                track: self.suggesting || self.collab.role == CollabRole::Suggest,
                api: false,
            },
        }
    }

    /// Run `f` as the relay's HTTP API's edit by `author` (its name and initials; not this
    /// person's to undo). This person's own name is theirs again afterwards.
    pub fn as_api_author<R>(&mut self, author: &str, f: impl FnOnce(&mut Session) -> R) -> R {
        let before = self.collab.api_author.replace(author.to_string());
        let r = f(self);
        self.collab.api_author = before;
        r
    }

    /// Where the active workbook's selection is.
    pub fn collab_place(&self) -> Option<SheetPlace> {
        let d = self.active()?;
        Some(SheetPlace { sheet: d.wb.active()?.name.clone(), selection: d.selection.clone() })
    }

    /// Put the active workbook's selection at `place` (its sheet becomes the active one). A
    /// sheet that is gone gives the first one: every replica must run an edit on the same sheet,
    /// whichever one its window last showed.
    pub fn set_collab_place(&mut self, place: &SheetPlace) {
        let Some(d) = self.active_mut() else { return };
        let i = d.wb.sheets.iter().position(|s| s.name == place.sheet).unwrap_or(0);
        d.wb.set_active_sheet(i);
        d.selection = place.selection.clone();
    }

    /// Run a command under `ctx`, as every replica runs an edit: its author, its clock and its
    /// random numbers.
    ///
    /// The edit came from someone else, through the relay: what co-editing refuses (code,
    /// macros, swapping the workbook, undo of this session's own history) is refused here too,
    /// and nothing it runs reaches this computer's files.
    pub fn execute_with(&mut self, ctx: &RunCtx, id: &str, params: Json) -> Result<Json> {
        if disabled(id) || COLLAB_UNDO.contains(&id) {
            return Err(EngineError::Disabled(id.to_string(), "This isn't available while co-editing".into()));
        }
        let sandbox = crate::io::ScriptSandbox::enter();
        let result = self.run_under(ctx, id, params);
        drop(sandbox);
        // A replayed edit opens no dialogs on this replica.
        self.ui_requests.clear();
        result
    }

    /// Run a command with `ctx`'s author, clock and random numbers.
    fn run_under(&mut self, ctx: &RunCtx, id: &str, params: Json) -> Result<Json> {
        let author = std::mem::replace(&mut self.prefs.user_name, ctx.author.clone());
        if let Some(d) = self.active_mut() {
            d.calc.reseed(seed(ctx));
        }
        let busy = std::mem::replace(&mut self.collab.busy, true);
        let track = std::mem::replace(&mut self.track_ctx, ctx.track.then(|| (ctx.author.clone(), ctx.time_ms)));
        let now = gridcraft_core::date::serial_from_unix_ms(ctx.time_ms as f64);
        let result = gridcraft_calc::with_now(now, || self.execute(id, params));
        self.track_ctx = track;
        self.collab.busy = busy;
        self.prefs.user_name = author;
        result
    }

    /// Whether `execute` must go through [`Session::execute_collab`].
    pub(crate) fn collab_wraps(&self) -> bool {
        self.collab.on && !self.collab.busy && self.depth == 0
    }

    /// [`Session::execute`] in co-editing mode, for a top-level command: refuses what co-editing
    /// can't do and records the command when it changed the workbook.
    pub(crate) fn execute_collab(&mut self, id: &str, params: Json) -> Result<Json> {
        let disabled_err = |why: &str| EngineError::Disabled(id.to_string(), format!("{why} isn't available while co-editing"));
        if disabled(id) {
            return Err(disabled_err("This"));
        }
        let person = self.collab.recording;
        if COLLAB_UNDO.contains(&id) && person {
            self.may_change(id)?;
            let place = self.collab_place();
            let ctx = self.run_ctx();
            self.collab.outbox.push(Recorded { id: id.to_string(), params, place, ctx });
            return Ok(serde_json::json!({"queued": true}));
        }
        // Every copy replays the edit, and they can't read this computer's files.
        if person && params.get("path").is_some_and(|p| !p.is_null()) && !id.starts_with("file.save") && !id.starts_with("file.export") {
            return Err(disabled_err("Reading a file from this computer"));
        }
        if !person {
            self.collab.busy = true;
            let r = self.execute(id, params);
            self.collab.busy = false;
            return r;
        }
        let before = self.active().map(|d| (d.uid, d.wb.clone()));
        let place = self.collab_place();
        let ctx = self.run_ctx();
        let result = self.run_under(&ctx, id, params.clone());
        let changed = match (&before, self.active()) {
            // A structural edit moves people's positions even where it changed no content (rows
            // inserted into an empty sheet).
            (Some((uid, wb)), Some(d)) => {
                d.uid == *uid && !std::sync::Arc::ptr_eq(wb, &d.wb) && (!wb.same_content(&d.wb) || wb.structure.len() != d.wb.structure.len())
            }
            _ => false,
        };
        if result.is_ok() && changed {
            // What the person may not change is put back: the edit never happened.
            if let Err(e) = self.may_change(id) {
                if let (Some((_, wb)), Some(d)) = (before, self.active_mut()) {
                    d.calc.rebuild(&wb);
                    d.wb = wb;
                }
                return Err(e);
            }
            self.collab.outbox.push(Recorded { id: id.to_string(), params, place, ctx });
        }
        result
    }
}

/// The first letters of a name's words ("Report Bot" → "RB"), as the HTTP API's edits are
/// initialled.
fn initials_of(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).collect()
}

impl Session {
    /// Whose suggestion an edit made now is and when: a replayed edit's with `track` on, or this
    /// person's own in Suggesting mode (not co-editing: there the edit's context says).
    pub(crate) fn track_now(&self) -> Option<(String, u64)> {
        if let Some(t) = &self.track_ctx {
            return Some(t.clone());
        }
        (self.suggesting && !self.collab.on).then(|| (self.prefs.user_name.clone(), self.run_ctx().time_ms))
    }

    /// Runs edit `id` as `author`'s suggestion: what it changes in cells is recorded with what the
    /// cells held ([`crate::suggest`]). What isn't an edit, and the review commands, run as they
    /// are; an edit that can't be suggested is refused.
    pub(crate) fn execute_tracked(&mut self, id: &str, params: Json, author: &str, time_ms: u64) -> Result<Json> {
        let edits = crate::cmd::find_command(id).is_some_and(|spec| spec.undoable);
        let run = |s: &mut Session, params: Json| {
            s.track_busy = true;
            let r = s.execute(id, params);
            s.track_busy = false;
            r
        };
        if !edits || crate::suggest::REVIEW_COMMANDS.contains(&id) {
            return run(self, params);
        }
        if !crate::suggest::SUGGEST_COMMANDS.contains(&id) {
            return Err(EngineError::Disabled(id.to_string(), "This can't be suggested: only changes to cells' contents and formatting can".into()));
        }
        let before = self.active().map(|d| (d.uid, d.wb.clone()));
        let result = run(self, params)?;
        let Some((uid, before)) = before else { return Ok(result) };
        let Some(d) = self.active_mut().filter(|d| d.uid == uid && !std::sync::Arc::ptr_eq(&before, &d.wb)) else { return Ok(result) };
        let mut wb = (*d.wb).clone();
        match crate::suggest::record(&before, &mut wb, author, time_ms) {
            Ok(()) => {
                d.wb = std::sync::Arc::new(wb);
                Ok(result)
            }
            Err(e) => {
                d.calc.rebuild(&before);
                d.wb = before;
                Err(e)
            }
        }
    }
}
