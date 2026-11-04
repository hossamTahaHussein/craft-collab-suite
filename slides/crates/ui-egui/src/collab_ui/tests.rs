//! Co-editing through the drawn interface: Version history from the status bar to Restore, the
//! title bar's save indicator and role, and undo while co-editing. The host side (the relay's
//! list, preview and restore) is the web app's; here it is played by filling `app.collab`.

use std::sync::Arc;

use deckcraft_engine::Session;
use deckcraft_engine::collab::CollabRole;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::*;
use crate::Services;

fn shared_app() -> SlideApp {
    let mut session = Session::with_new();
    session.set_collab(true);
    let mut app = SlideApp::new(session, Services::default());
    app.collab.active = true;
    app
}

fn harness(app: SlideApp) -> Harness<'static, SlideApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 800.0)).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

/// A presentation of `n` slides.
fn deck(n: usize) -> Arc<Presentation> {
    let mut s = Session::with_new();
    for _ in 1..n {
        s.execute("slide.new", &serde_json::json!({})).unwrap();
    }
    s.active().unwrap().doc.clone()
}

#[test]
fn version_history_from_the_status_bar_to_restore() {
    let mut h = harness(shared_app());
    h.get_by_label("Version history").click();
    h.run_steps(3);
    assert!(h.state_mut().take_collab_requests().contains(&CollabRequest::ListVersions), "the list is asked for");
    h.state_mut().collab.set_versions(vec![VersionRow { rev: 7, at: "2026-10-30T09:15:00Z".into(), users: vec!["Ann Lee".into()] }]);
    h.run_steps(3);
    let row = h.query_all_by(|n| n.label().is_some_and(|l| l.contains("Ann Lee"))).next().expect("the version is listed");
    row.click();
    h.run_steps(3);
    assert_eq!(h.state_mut().take_collab_requests(), [CollabRequest::Preview(7)], "picking it asks for it");
    h.state_mut().collab.set_preview(7, DeckPreview::new(Some(deck(2))));
    h.run_steps(3);
    assert!(h.query_by_label("Kept slide 2").is_some(), "the kept slides show");
    h.get_by_label("Restore this version").click();
    h.run_steps(3);
    assert_eq!(h.state_mut().take_collab_requests(), [CollabRequest::Restore(7)]);
    assert!(h.state().collab.history.is_none(), "the window closes");
}

#[test]
fn version_history_is_offered_only_while_co_editing_and_viewers_cannot_restore() {
    let mut app = shared_app();
    app.collab.active = false;
    app.session.set_collab(false);
    let h = harness(app);
    assert!(h.query_by_label("Version history").is_none());
    let mut app = shared_app();
    app.session.set_collab_role(CollabRole::View);
    app.collab.history = Some(VersionHistory { rows: Some(Vec::new()), preview: Some((7, Some(DeckPreview::new(Some(deck(1)))))) });
    let h = harness(app);
    assert!(h.query_by_label("Kept slide 1").is_some());
    assert!(h.query_by_label("Restore this version").is_none());
}

#[test]
fn a_version_previews_as_its_slides() {
    let ctx = egui::Context::default();
    assert_eq!(DeckPreview::new(Some(deck(3))).slides(&ctx, PREVIEW_WIDTH).len(), 3);
    assert!(DeckPreview::new(None).slides(&ctx, PREVIEW_WIDTH).is_empty());
}

#[test]
fn the_title_bar_says_whether_changes_are_saved_and_the_role() {
    let mut app = shared_app();
    app.session.set_collab_role(CollabRole::Comment);
    let mut h = harness(app);
    for state in [SyncState::Saving, SyncState::Saved, SyncState::Offline] {
        h.state_mut().collab.sync = Some(state);
        h.run_steps(2);
        let at = h.get_by_label(state.label()).rect();
        assert!(at.max.y < 40.0, "{} is in the title bar: {at:?}", state.label());
    }
    assert!(h.query_by_label("Commenting").is_some());
}

#[test]
fn undo_while_co_editing_is_the_hosts_and_never_a_viewers() {
    let mut app = shared_app();
    assert_eq!(app.collab_undo_state(), Some((false, false)));
    assert!(!crate::qat::is_enabled(&app, "edit.undo"));
    app.collab.can_undo = true;
    assert_eq!(app.collab_undo_state(), Some((true, false)));
    assert!(crate::qat::is_enabled(&app, "edit.undo"));
    app.session.set_collab_role(CollabRole::View);
    assert_eq!(app.collab_undo_state(), Some((false, false)));
    app.collab.active = false;
    assert_eq!(app.collab_undo_state(), None, "not co-editing: the presentation's own undo");
}

#[test]
fn the_read_only_notice_offers_a_copy() {
    let mut app = shared_app();
    app.collab.offline = true;
    let mut h = harness(app);
    h.get_by_label("Save my copy").click();
    h.run_steps(3);
    assert_eq!(h.state_mut().take_collab_requests(), [CollabRequest::SaveMyCopy]);
}
