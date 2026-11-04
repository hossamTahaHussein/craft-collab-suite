//! The windows as drawn: what shows, and what a click asks the host for.

use egui::epaint::{ClippedShape, Shape};
use egui::{Context, Event, PointerButton, Pos2, RawInput, Rect, vec2};

use super::*;

/// A preview that shows one line of text.
struct Text(Option<&'static str>);

impl Preview for Text {
    fn kept(&self) -> bool {
        self.0.is_some()
    }
    fn show(&mut self, ui: &mut egui::Ui) {
        ui.label(self.0.unwrap_or_default());
    }
}

fn shared() -> CollabUi<Text> {
    CollabUi { active: true, ..CollabUi::default() }
}

fn tr(s: &str) -> String {
    s.to_string()
}

/// One frame of `collab` (an editor when `editor`), with `events`; its shapes.
fn frame(ctx: &Context, collab: &mut CollabUi<Text>, editor: bool, events: Vec<Event>) -> Vec<ClippedShape> {
    let input = |events| RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), events, ..RawInput::default() };
    // A window is sized, not drawn, on its first frame.
    let mut out = ctx.run_ui(input(Vec::new()), |ui| show(collab, ui.ctx(), editor, &tr));
    out.textures_delta.clear();
    let mut out = ctx.run_ui(input(events), |ui| show(collab, ui.ctx(), editor, &tr));
    out.textures_delta.clear();
    out.shapes
}

fn text_rect(shapes: &[ClippedShape], part: &str) -> Option<Rect> {
    fn find(shape: &Shape, part: &str) -> Option<Rect> {
        match shape {
            Shape::Text(t) if t.galley.text().contains(part) => Some(t.visual_bounding_rect()),
            Shape::Vec(v) => v.iter().find_map(|s| find(s, part)),
            _ => None,
        }
    }
    shapes.iter().find_map(|c| find(&c.shape, part))
}

/// Clicks `part` (drawn in the frame before), over the next frames.
fn click(ctx: &Context, collab: &mut CollabUi<Text>, editor: bool, part: &str) {
    let shapes = frame(ctx, collab, editor, Vec::new());
    let at = text_rect(&shapes, part).unwrap_or_else(|| panic!("{part} is drawn")).center();
    let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
    for events in [vec![Event::PointerMoved(at), press(true)], vec![press(false)]] {
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), events, ..RawInput::default() };
        ctx.run_ui(input, |ui| show(collab, ui.ctx(), editor, &tr)).textures_delta.clear();
    }
}

#[test]
fn version_history_opens_only_while_co_editing() {
    let mut c = CollabUi::<Text>::default();
    assert!(!c.open_history());
    assert!(c.history.is_none() && c.take_requests().is_empty());
    let mut c = shared();
    assert!(c.open_history());
    assert_eq!(c.take_requests(), [CollabRequest::ListVersions]);
}

#[test]
fn the_save_indicator_follows_the_link() {
    assert_eq!(SyncState::of(false, 3), SyncState::Offline);
    assert_eq!(SyncState::of(true, 1), SyncState::Saving);
    assert_eq!(SyncState::of(true, 0), SyncState::Saved);
    assert_eq!(SyncState::Saved.label(), "All changes saved");
    assert_eq!(SyncState::Offline.label(), "Offline — changes will sync");
}

/// Read only for edits piling up while connected isn't being offline: the notice says which.
#[test]
fn the_read_only_notice_says_why_and_saves_a_copy() {
    let (title, text) = read_only_notice(false);
    assert_eq!(title, "Reconnecting…");
    assert!(text.contains("offline"), "{text}");
    let (title, text) = read_only_notice(true);
    assert_eq!(title, "Syncing…");
    assert!(text.contains("waiting to sync") && !text.contains("offline"), "{text}");
    let ctx = Context::default();
    let mut c = CollabUi { offline: true, ..shared() };
    assert!(text_rect(&frame(&ctx, &mut c, true, Vec::new()), "Reconnecting…").is_some());
    click(&ctx, &mut c, true, "Save my copy");
    assert_eq!(c.take_requests(), [CollabRequest::SaveMyCopy]);
    c.offline = false;
    assert!(text_rect(&frame(&ctx, &mut c, true, Vec::new()), "Save my copy").is_none(), "gone once back in sync");
}

#[test]
fn a_version_from_the_list_to_restore() {
    let ctx = Context::default();
    let mut c = shared();
    c.open_history();
    c.take_requests();
    assert!(text_rect(&frame(&ctx, &mut c, true, Vec::new()), "Loading…").is_some());
    c.set_versions(vec![VersionRow { rev: 7, at: "2026-10-30T09:15:00Z".into(), users: vec!["Ann Lee".into()] }]);
    click(&ctx, &mut c, true, "Ann Lee");
    assert_eq!(c.take_requests(), [CollabRequest::Preview(7)], "picking it asks for it");
    // An answer for another version is dropped; this one's preview shows.
    c.set_preview(8, Text(Some("not this one")));
    assert!(matches!(c.history.as_ref().unwrap().preview, Some((7, None))));
    c.set_preview(7, Text(Some("An older draft")));
    assert!(text_rect(&frame(&ctx, &mut c, true, Vec::new()), "An older draft").is_some());
    click(&ctx, &mut c, true, "Restore this version");
    assert_eq!(c.take_requests(), [CollabRequest::Restore(7)]);
    assert!(c.history.is_none(), "the window closes");
}

#[test]
fn a_viewer_previews_but_cannot_restore_and_a_lost_version_says_so() {
    let ctx = Context::default();
    let mut c = shared();
    c.history = Some(VersionHistory { rows: Some(Vec::new()), preview: Some((7, Some(Text(Some("An older draft"))))) });
    let shapes = frame(&ctx, &mut c, false, Vec::new());
    assert!(text_rect(&shapes, "An older draft").is_some());
    assert!(text_rect(&shapes, "No versions yet").is_some());
    assert!(text_rect(&shapes, "Restore this version").is_none());
    c.history.as_mut().unwrap().preview = Some((7, Some(Text(None))));
    let shapes = frame(&ctx, &mut c, true, Vec::new());
    assert!(text_rect(&shapes, "no longer kept").is_some());
    assert!(text_rect(&shapes, "Restore this version").is_none());
}

/// Version history opens at the reading start: the top right in a right-to-left interface.
#[test]
fn version_history_opens_at_the_reading_start() {
    for rtl in [false, true] {
        let ctx = Context::default();
        craft_ui::direction::apply(&ctx, if rtl { craft_ui::direction::Direction::Rtl } else { craft_ui::direction::Direction::Ltr });
        let mut c = shared();
        c.history = Some(VersionHistory { rows: Some(Vec::new()), preview: None });
        let at = text_rect(&frame(&ctx, &mut c, true, Vec::new()), "Version history").expect("the title");
        assert_eq!(at.center().x > 600.0, rtl, "rtl {rtl}: {at:?}");
    }
}
