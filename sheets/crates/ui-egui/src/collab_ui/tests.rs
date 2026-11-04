//! Co-editing through the drawn interface: Version history from the status bar to Restore, the
//! title bar's save indicator and role, and undo while co-editing. The host side (the relay's
//! list, preview and restore) is the web app's; here it is played by filling `app.collab`.

use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use gridcraft_engine::collab::CollabRole;
use serde_json::json;

use super::*;
use crate::Services;

fn shared_app() -> SheetApp {
    let mut session = gridcraft_engine::Session::new();
    session.new_workbook();
    session.set_collab(true);
    let mut app = SheetApp::new(session, Services::default());
    app.use_prepared_fonts();
    app.collab.active = true;
    app
}

fn harness(app: SheetApp) -> Harness<'static, SheetApp> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_context_setup(|ctx| {
            ctx.set_fonts(crate::theme::pinned_font_definitions());
            crate::theme::apply(ctx, false);
        })
        .renderer(egui_kittest::LazyRenderer::Uninitialized { textures_delta: Default::default(), builder: None })
        .build_ui_state(
            |ui, app: &mut SheetApp| {
                app.logic(ui.ctx());
                app.ui(ui);
            },
            app,
        );
    h.run_steps(4);
    h
}

/// A workbook whose A1 says `text`.
fn workbook(text: &str) -> Arc<Workbook> {
    let mut s = gridcraft_engine::Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": text})).unwrap();
    s.active().unwrap().wb.clone()
}

#[test]
fn version_history_from_the_status_bar_to_restore() {
    let mut h = harness(shared_app());
    h.get_by_label("Version history").click();
    h.run_steps(3);
    assert!(h.state_mut().take_collab_requests().contains(&CollabRequest::ListVersions), "the list is asked for");
    // The host answers with the relay's list.
    h.state_mut().collab.set_versions(vec![VersionRow { rev: 7, at: "2026-10-30T09:15:00Z".into(), users: vec!["Ann Lee".into()] }]);
    h.run_steps(3);
    let row = h.query_all_by(|n| n.label().is_some_and(|l| l.contains("Ann Lee"))).next().expect("the version is listed");
    row.click();
    h.run_steps(3);
    assert_eq!(h.state_mut().take_collab_requests(), [CollabRequest::Preview(7)], "picking it asks for it");
    // The host answers with the version's workbook: its cells show.
    h.state_mut().collab.set_preview(7, WorkbookPreview::new(Some(workbook("An older value"))));
    h.run_steps(3);
    assert!(h.query_by_label("An older value").is_some(), "the kept cells show");
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
    app.collab.history = Some(VersionHistory { rows: Some(Vec::new()), preview: Some((7, Some(WorkbookPreview::new(Some(workbook("Kept")))))) });
    let h = harness(app);
    assert!(h.query_by_label("Kept").is_some());
    assert!(h.query_by_label("Restore this version").is_none());
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
    app.collab.can_undo = true;
    assert_eq!(app.collab_undo_state(), Some((true, false)));
    app.session.set_collab_role(CollabRole::View);
    assert_eq!(app.collab_undo_state(), Some((false, false)));
    app.collab.active = false;
    assert_eq!(app.collab_undo_state(), None, "not co-editing: the workbook's own undo");
}

#[test]
fn the_read_only_notice_offers_a_copy() {
    let mut app = shared_app();
    app.collab.offline = true;
    let mut h = harness(app);
    assert!(h.query_by_label("Reconnecting…").is_some() || h.query_all_by(|n| n.label().is_some_and(|l| l.contains("offline"))).next().is_some());
    h.get_by_label("Save my copy").click();
    h.run_steps(3);
    assert_eq!(h.state_mut().take_collab_requests(), [CollabRequest::SaveMyCopy]);
}
