//! A Craft Slides presentation as the suite's co-editing link layer sees it (`craft_collab_link`).

use std::sync::Arc;

use craft_collab_core::doc::CollabDoc;
use craft_collab_link::LinkHost;
use deckcraft_engine::Session;
use deckcraft_engine::collab::{CollabRole, SlidePlace};
use deckcraft_model::Presentation;

use crate::adapter::{SlideSel, SlidesDoc, text_body};
use crate::change::pos_at;

/// The page ↔ replica glue for a presentation.
pub type CollabLink = craft_collab_link::CollabLink<SlidesDoc>;
pub type PeerView = craft_collab_link::PeerView<SlidePlace>;
pub use craft_collab_link::{HostRequest, OFFLINE_LIMIT_MS, PENDING_LIMIT, Role};

impl LinkHost for SlidesDoc {
    type Shown = SlidePlace;

    fn decode(text: &str) -> Option<Arc<Presentation>> {
        crate::decode(text).map(Arc::new)
    }

    fn document_id(s: &Session) -> u64 {
        s.active().map_or(0, |d| d.uid)
    }

    fn opened(s: &Session) -> Arc<Presentation> {
        Self::doc(s).clone()
    }

    fn show_shared(s: &mut Session, doc: Arc<Presentation>, name: Option<&str>) {
        s.set_collab_presentation(doc);
        if let Some(d) = s.active_mut() {
            d.path = name.map(str::to_string);
            d.title = name.unwrap_or("Shared presentation").to_string();
            d.saved_doc = d.doc.clone();
        }
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

    /// The presentation's version: every edit makes a new one.
    fn visible_rev(s: &Session) -> u64 {
        Arc::as_ptr(Self::doc(s)) as u64
    }

    fn set_collab(s: &mut Session, on: bool) {
        s.set_collab(on);
    }

    /// Their slide, and their text caret as a paragraph and character in the visible text.
    fn show(s: &Session, sel: &SlideSel) -> Option<SlidePlace> {
        let doc = Self::doc(s);
        doc.slides.iter().any(|sl| sl.id == sel.place.slide).then_some(())?;
        let mut place = sel.place.clone();
        if let (Some((key, a, c)), Some(t)) = (sel.text, place.selection.text.as_mut()) {
            let body = text_body(doc, key)?;
            t.anchor = pos_at(body, a);
            t.caret = pos_at(body, c);
            t.line_end = None;
        }
        Some(place)
    }
}
