//! WordCraft as the suite's co-editing link layer sees it (`craft_collab_link`): snapshots are
//! native JSON, positions are drawn as selections, and Viewing is the read-only lock.

use craft_collab_link::LinkHost;
use wordcraft_doc::Document;
use wordcraft_doc::offsets::pos_at;
use wordcraft_engine::{CollabRole, Selection, Session};

use crate::adapter::DocsDoc;
use crate::proto::Anchor;

/// The page ↔ replica glue for a WordCraft document.
pub type CollabLink = craft_collab_link::CollabLink<DocsDoc>;
pub type PeerView = craft_collab_link::PeerView<Selection>;
pub use craft_collab_link::{HostRequest, OFFLINE_LIMIT_MS, PENDING_LIMIT, Role};

impl LinkHost for DocsDoc {
    type Shown = Selection;

    fn decode(json: &str) -> Option<Document> {
        wordcraft_engine::io::open_bytes("shared.json", json.as_bytes()).ok()
    }

    fn document_id(s: &Session) -> u64 {
        s.document_id()
    }

    fn opened(s: &Session) -> Document {
        s.doc.clone()
    }

    fn show_shared(s: &mut Session, doc: Document, name: Option<&str>) {
        s.set_document(doc);
        s.path = name.map(std::path::PathBuf::from);
        s.dirty = false;
    }

    fn set_author(s: &mut Session, name: &str) {
        s.author = name.to_string();
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
        (s.viewing, s.viewing_locked)
    }

    fn set_read_only(s: &mut Session, (viewing, locked): (bool, bool)) {
        s.viewing = viewing;
        s.viewing_locked = locked;
    }

    fn visible_rev(s: &Session) -> u64 {
        s.rev()
    }

    fn set_collab(s: &mut Session, on: bool) {
        s.set_collab(on);
    }

    fn show(s: &Session, a: &Anchor) -> Option<Selection> {
        s.doc.story(a.story)?;
        Some(Selection { anchor: pos_at(&s.doc, a.story, a.anchor), focus: pos_at(&s.doc, a.story, a.head) })
    }
}
