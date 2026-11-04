//! The co-editing UI Craft Sheets and Craft Slides share, as Craft Docs draws it (co-editing
//! spec §7, §9): the header's save indicator, the read-only notice ("Reconnecting…" with "Save
//! my copy") and the Version history window (list, read-only preview, "Restore this version" for
//! editors). The host fills [`CollabUi`] from its co-editing link and takes back
//! [`CollabRequest`]s; this crate draws only, and each editor draws its own preview
//! ([`Preview`]) and translates the words (`tr`).

use egui::Context;

mod time;

pub use time::utc_time;

/// What the co-editing host tells the UI each frame.
#[derive(Debug)]
pub struct CollabUi<P> {
    /// The document is shared (co-editing is on).
    pub active: bool,
    /// The person has an edit of theirs to undo, or an undone one to redo.
    pub can_undo: bool,
    pub can_redo: bool,
    /// Too long offline or too many edits waiting: the document is read only until they sync.
    pub offline: bool,
    /// Connected to the relay (so a read-only document is waiting on its edits, not offline).
    pub connected: bool,
    /// The header's save indicator; `None` when not co-editing.
    pub sync: Option<SyncState>,
    /// The Version history window, when open.
    pub history: Option<VersionHistory<P>>,
    /// What the person asked for, for the host.
    pub requests: Vec<CollabRequest>,
}

impl<P> Default for CollabUi<P> {
    fn default() -> Self {
        CollabUi {
            active: false,
            can_undo: false,
            can_redo: false,
            offline: false,
            connected: false,
            sync: None,
            history: None,
            requests: Vec::new(),
        }
    }
}

/// Whether this person's edits have reached the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncState {
    /// Edits on their way to the relay.
    Saving,
    /// Everything confirmed.
    Saved,
    /// Not connected: edits wait and sync on reconnecting.
    Offline,
}

impl SyncState {
    /// From the link: connected or not, and how many edits wait for the relay.
    pub fn of(online: bool, pending: usize) -> SyncState {
        if !online {
            SyncState::Offline
        } else if pending > 0 {
            SyncState::Saving
        } else {
            SyncState::Saved
        }
    }

    /// The indicator's words (English; the editor translates them).
    pub fn label(self) -> &'static str {
        match self {
            SyncState::Saving => "Saving…",
            SyncState::Saved => "All changes saved",
            SyncState::Offline => "Offline — changes will sync",
        }
    }
}

#[derive(Debug)]
pub struct VersionHistory<P> {
    /// The versions, newest first; `None` while they load.
    pub rows: Option<Vec<VersionRow>>,
    /// The version being previewed: its revision and its preview (`None` while it loads).
    pub preview: Option<(u64, Option<P>)>,
}

impl<P> Default for VersionHistory<P> {
    fn default() -> Self {
        VersionHistory { rows: None, preview: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionRow {
    pub rev: u64,
    /// When it was kept (ISO time, UTC).
    pub at: String,
    /// Who edited since the version before.
    pub users: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollabRequest {
    ListVersions,
    Preview(u64),
    Restore(u64),
    /// Save this person's copy of the document (offline), as "<name> (my copy).<ext>".
    SaveMyCopy,
}

/// A kept version as the editor shows it, read only.
pub trait Preview {
    /// False when the version is gone or can't be read.
    fn kept(&self) -> bool;
    /// Draws the version (its pages, cells or slides), in a scroll area of the window.
    fn show(&mut self, ui: &mut egui::Ui);
}

impl<P> CollabUi<P> {
    /// Version history while co-editing: open the window and ask for the list. False (and
    /// nothing asked) when the document isn't shared.
    pub fn open_history(&mut self) -> bool {
        if !self.active {
            return false;
        }
        self.history = Some(VersionHistory::default());
        self.requests.push(CollabRequest::ListVersions);
        true
    }

    /// The host's requests since the last call.
    pub fn take_requests(&mut self) -> Vec<CollabRequest> {
        std::mem::take(&mut self.requests)
    }

    /// The relay's list of versions, for an open window.
    pub fn set_versions(&mut self, rows: Vec<VersionRow>) {
        if let Some(h) = self.history.as_mut() {
            h.rows = Some(rows);
        }
    }

    /// A version's preview, when it is the one the window waits for.
    pub fn set_preview(&mut self, rev: u64, preview: P) {
        if let Some(h) = self.history.as_mut()
            && h.preview.as_ref().is_some_and(|(r, _)| *r == rev)
        {
            h.preview = Some((rev, Some(preview)));
        }
    }
}

/// The read-only notice's title and text: offline, or connected with too many edits waiting.
pub fn read_only_notice(connected: bool) -> (&'static str, &'static str) {
    if connected {
        ("Syncing…", "Too many changes are waiting to sync. The document is read only until they do.")
    } else {
        ("Reconnecting…", "You're offline. The document is read only until your changes sync.")
    }
}

/// Draws the read-only notice and the Version history window, when they are up. `editor`: the
/// person may restore a version. `tr` translates the English words into the interface language.
pub fn show<P: Preview>(collab: &mut CollabUi<P>, ctx: &Context, editor: bool, tr: &dyn Fn(&str) -> String) {
    if collab.offline {
        offline_notice(collab, ctx, tr);
    }
    if collab.history.is_some() {
        version_history(collab, ctx, editor, tr);
    }
}

fn offline_notice<P>(collab: &mut CollabUi<P>, ctx: &Context, tr: &dyn Fn(&str) -> String) {
    let (title, text) = read_only_notice(collab.connected);
    egui::Window::new(tr(title))
        .id(egui::Id::unique("craft_collab_offline"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .show(ctx, |ui| {
            ui.label(tr(text));
            if ui.button(tr("Save my copy")).clicked() {
                collab.requests.push(CollabRequest::SaveMyCopy);
            }
        });
}

fn version_history<P: Preview>(collab: &mut CollabUi<P>, ctx: &Context, editor: bool, tr: &dyn Fn(&str) -> String) {
    let mut open = true;
    let mut asked = Vec::new();
    // It opens at the reading start: the top right in a right-to-left interface.
    let screen = ctx.content_rect();
    let (pivot, at) = if craft_ui::direction::is_rtl(ctx) {
        (egui::Align2::RIGHT_TOP, egui::pos2(screen.right() - 16.0, screen.top() + 16.0))
    } else {
        (egui::Align2::LEFT_TOP, egui::pos2(screen.left() + 16.0, screen.top() + 16.0))
    };
    egui::Window::new(tr("Version history"))
        .id(egui::Id::unique("craft_collab_versions"))
        .pivot(pivot)
        .default_pos(at)
        .open(&mut open)
        .default_width(420.0)
        .default_height(420.0)
        .show(ctx, |ui| {
            let Some(history) = collab.history.as_mut() else { return };
            let picked_rev = history.preview.as_ref().map(|(r, _)| *r);
            match &history.rows {
                None => {
                    ui.label(tr("Loading…"));
                }
                Some(rows) if rows.is_empty() => {
                    ui.label(tr("No versions yet. A version is kept when the document is saved."));
                }
                Some(rows) => {
                    egui::ScrollArea::vertical().id_salt("craft_collab_version_rows").max_height(180.0).show(ui, |ui| {
                        for row in rows {
                            let text = format!("{}  {}", short_time(&row.at), row.users.join(", "));
                            if ui.selectable_label(picked_rev == Some(row.rev), text.trim_end()).clicked() {
                                asked.push(CollabRequest::Preview(row.rev));
                            }
                        }
                    });
                }
            }
            if let Some((rev, preview)) = history.preview.as_mut() {
                ui.separator();
                match preview {
                    None => {
                        ui.label(tr("Loading…"));
                    }
                    Some(p) if !p.kept() => {
                        ui.label(tr("This version is no longer kept."));
                    }
                    Some(p) => {
                        if editor && ui.button(tr("Restore this version")).clicked() {
                            asked.push(CollabRequest::Restore(*rev));
                        }
                        egui::ScrollArea::vertical().id_salt("craft_collab_version_preview").max_height(420.0).show(ui, |ui| p.show(ui));
                    }
                }
            }
        });
    for request in asked {
        if let (CollabRequest::Preview(rev), Some(h)) = (&request, collab.history.as_mut()) {
            h.preview = Some((*rev, None));
        }
        if matches!(request, CollabRequest::Restore(_)) {
            collab.history = None;
        }
        collab.requests.push(request);
    }
    if !open {
        collab.history = None;
    }
}

/// A version's time in local time (the relay keeps UTC); as written when it isn't a time.
fn short_time(iso: &str) -> String {
    let t = utc_time(iso);
    if t.is_empty() { iso.to_string() } else { t }
}

#[cfg(test)]
mod tests;
