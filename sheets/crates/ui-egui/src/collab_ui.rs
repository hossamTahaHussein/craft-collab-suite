//! Co-editing UI for a shared workbook (co-editing spec §7, §9), as Craft Docs has it: the save
//! indicator and the role in the title bar, Version history from the status bar (a kept version
//! previews as its cells, read only; editors restore it) and the read-only notice. The windows
//! are `craft_collab_ui`'s; the host fills [`SheetApp::collab`] from its co-editing link.

use std::sync::Arc;

use gridcraft_engine::collab::CollabRole;
use gridcraft_engine::core::CellRef;
use gridcraft_engine::model::Workbook;

pub use craft_collab_ui::{CollabRequest, SyncState, VersionHistory, VersionRow};

use crate::SheetApp;

/// The co-editing state the host keeps up to date each frame.
pub type CollabUi = craft_collab_ui::CollabUi<WorkbookPreview>;

/// The rows and columns of each sheet a version's preview shows.
const PREVIEW_ROWS: u32 = 40;
const PREVIEW_COLS: u32 = 12;

/// A kept version of the workbook, shown as its sheets' cells.
#[derive(Debug)]
pub struct WorkbookPreview(Option<Arc<Workbook>>);

impl WorkbookPreview {
    /// `None` when the version is gone or can't be read.
    pub fn new(wb: Option<Arc<Workbook>>) -> WorkbookPreview {
        WorkbookPreview(wb)
    }
}

impl craft_collab_ui::Preview for WorkbookPreview {
    fn kept(&self) -> bool {
        self.0.is_some()
    }

    fn show(&mut self, ui: &mut egui::Ui) {
        let Some(wb) = &self.0 else { return };
        for (i, sheet) in wb.sheets.iter().enumerate() {
            ui.strong(&sheet.name);
            let Some(used) = sheet.used_range() else {
                ui.weak("—");
                continue;
            };
            let last_row = used.end.row.min(PREVIEW_ROWS.saturating_sub(1));
            let last_col = used.end.col.min(PREVIEW_COLS.saturating_sub(1));
            egui::Grid::new(("collab_version_sheet", i)).striped(true).show(ui, |ui| {
                ui.label("");
                for col in 0..=last_col {
                    ui.weak(gridcraft_engine::core::col_to_letters(col));
                }
                ui.end_row();
                for row in 0..=last_row {
                    ui.weak((row + 1).to_string());
                    for col in 0..=last_col {
                        ui.label(gridcraft_engine::display::cell_text(wb, sheet, CellRef { row, col }));
                    }
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
        }
    }
}

impl SheetApp {
    /// Version history while co-editing: open the window and ask the host for the list.
    pub fn open_version_history(&mut self) {
        self.collab.open_history();
    }

    /// The host's co-editing requests since the last call.
    pub fn take_collab_requests(&mut self) -> Vec<CollabRequest> {
        self.collab.take_requests()
    }

    /// Undo and redo while co-editing are this person's own edits for everyone; the host says
    /// whether there is one, and viewers have none.
    pub(crate) fn collab_undo_state(&self) -> Option<(bool, bool)> {
        let viewer = self.session.collab_role() == CollabRole::View;
        self.collab.active.then_some((self.collab.can_undo && !viewer, self.collab.can_redo && !viewer))
    }
}

/// The role's label in the title bar: `None` for an editor.
pub(crate) fn role_label(app: &SheetApp) -> Option<&'static str> {
    if !app.session.collab() {
        return None;
    }
    match app.session.collab_role() {
        CollabRole::Edit => None,
        CollabRole::Comment => Some("Commenting"),
        CollabRole::Suggest => Some("Suggesting"),
        CollabRole::View => Some("View only"),
    }
}

/// The title bar's co-editing words: the save indicator and the role, translated.
pub(crate) fn title_words(app: &SheetApp) -> Vec<String> {
    let lang = app.ui.language;
    app.collab.sync.map(|s| lang.tr(s.label()).to_string()).into_iter().chain(role_label(app).map(|r| lang.tr(r).to_string())).collect()
}

pub(crate) fn show(app: &mut SheetApp, ctx: &egui::Context) {
    let lang = app.ui.language;
    let editor = app.session.collab_role() == CollabRole::Edit;
    craft_collab_ui::show(&mut app.collab, ctx, editor, &|s| lang.tr(s).to_string());
}

#[cfg(test)]
mod tests;
