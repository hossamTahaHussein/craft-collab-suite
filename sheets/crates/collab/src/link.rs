//! A Craft Sheets workbook as the suite's co-editing link layer sees it (`craft_collab_link`).

use std::sync::Arc;

use craft_collab_core::doc::CollabDoc;
use craft_collab_link::LinkHost;
use gridcraft_engine::Session;
use gridcraft_engine::collab::{CollabRole, SheetPlace};
use gridcraft_model::Workbook;

use crate::adapter::{SheetSel, SheetsDoc};

/// The page ↔ replica glue for a workbook.
pub type CollabLink = craft_collab_link::CollabLink<SheetsDoc>;
pub type PeerView = craft_collab_link::PeerView<SheetPlace>;
pub use craft_collab_link::{HostRequest, OFFLINE_LIMIT_MS, PENDING_LIMIT, Role};

impl LinkHost for SheetsDoc {
    type Shown = SheetPlace;

    fn decode(json: &str) -> Option<Arc<Workbook>> {
        crate::decode(json).map(Arc::new)
    }

    fn document_id(s: &Session) -> u64 {
        s.active().map_or(0, |d| d.uid)
    }

    fn opened(s: &Session) -> Arc<Workbook> {
        Self::doc(s).clone()
    }

    fn show_shared(s: &mut Session, doc: Arc<Workbook>, name: Option<&str>) {
        s.set_collab_workbook(doc);
        if let Some(d) = s.active_mut() {
            d.path = name.map(str::to_string);
            d.title = name.unwrap_or("Shared workbook").to_string();
            d.saved = d.wb.clone();
        }
    }

    fn set_author(s: &mut Session, name: &str) {
        s.prefs.user_name = name.to_string();
    }

    fn set_role(s: &mut Session, role: Role) {
        s.set_collab_role(match role {
            Role::Edit => CollabRole::Edit,
            Role::Suggest => CollabRole::Suggest,
            Role::Comment => CollabRole::Comment,
            Role::View => CollabRole::View,
        });
    }

    fn read_only(s: &Session) -> (bool, bool) {
        s.read_only()
    }

    fn set_read_only(s: &mut Session, (on, locked): (bool, bool)) {
        s.set_read_only(on, locked);
    }

    /// The workbook's version: every edit makes a new one.
    fn visible_rev(s: &Session) -> u64 {
        Arc::as_ptr(Self::doc(s)) as u64
    }

    fn set_collab(s: &mut Session, on: bool) {
        s.set_collab(on);
    }

    fn show(s: &Session, sel: &SheetSel) -> Option<SheetPlace> {
        Self::doc(s).sheets.iter().any(|sh| sh.name == sel.place.sheet).then(|| sel.place.clone())
    }
}
