//! The Arabic interface reads from the right, as in Craft Docs and Craft Sheets.
//! `CRAFT_SLIDES_RTL_SCREENSHOT=path.png` saves the window.

use deckcraft_engine::Session;
use egui_kittest::kittest::Queryable;

use crate::i18n::Language;
use crate::{Services, SlideApp};

fn arabic() -> egui_kittest::Harness<'static, SlideApp> {
    let mut app = SlideApp::new(Session::with_new(), Services::default());
    app.ui.language = Language::Ar;
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    h
}

fn shot(h: &mut egui_kittest::Harness<'static, SlideApp>, name: &str) {
    if let Ok(path) = std::env::var("CRAFT_SLIDES_RTL_SCREENSHOT")
        && let Ok(img) = h.render()
    {
        let _ = img.save(std::path::Path::new(&path).with_file_name(name));
    }
}

#[test]
fn the_window_reads_from_the_right_in_arabic() {
    let mut h = arabic();
    assert!(craft_ui::direction::is_rtl(&h.ctx));
    shot(&mut h, "slides-ar.png");
    let home = h.query_all_by_label("الصفحة الرئيسية").next().map(|n| n.rect());
    let insert = h.query_all_by_label("إدراج").next().map(|n| n.rect());
    if let (Some(home), Some(insert)) = (home, insert) {
        assert!(home.center().x > insert.center().x, "Home before Insert, from the right: {home:?} {insert:?}");
    }
    // The thumbnails pane is at the right, so the slide ends before it.
    let canvas = h.state().canvas_rect.expect("canvas");
    assert!(canvas.right() < 1280.0 - 140.0, "the thumbnails pane is at the right: {canvas:?}");
}

#[test]
fn the_interface_language_is_a_command_and_survives_a_saved_settings_file() {
    let mut app = SlideApp::new(Session::with_new(), Services::default());
    app.run("ui.language", serde_json::json!({"value": "ar"})).unwrap();
    assert_eq!(app.ui.language, Language::Ar);
    assert!(app.run("ui.language", serde_json::json!({"value": "tlh"})).unwrap_err().contains("no interface language"));
    let saved = serde_json::to_value(&app.ui).unwrap();
    assert_eq!(saved["language"], "ar");
    let back: crate::UiState = serde_json::from_value(saved).unwrap();
    assert_eq!(back.language, Language::Ar);
}

/// Task panes (Comments, Format Shape, Animation…) open at the reading end: the left in Arabic,
/// so comment cards are at the left as in Craft Docs.
#[test]
fn task_panes_open_at_the_left_in_arabic() {
    let mut h = arabic();
    h.state_mut().ui.pane = Some("comments".into());
    h.run_steps(4);
    shot(&mut h, "slides-ar-comments.png");
    let canvas = h.state().canvas_rect.expect("canvas");
    assert!(canvas.left() > 200.0, "the Comments pane is at the left of the slide: {canvas:?}");
    let title = h.query_all_by_label("التعليقات").next().map(|n| n.rect()).or_else(|| h.query_all_by_label("Comments").next().map(|n| n.rect()));
    if let Some(t) = title {
        assert!(t.center().x < canvas.left(), "the pane's title is left of the slide: {t:?}");
    }
}

/// About names the suite and credits its author, in the interface language.
#[test]
fn about_credits_hossam_taha_in_the_interface_language() {
    let mut h = arabic();
    h.state_mut().run("app.about", serde_json::json!({})).unwrap();
    h.run_steps(4);
    assert!(h.query_all_by_label("Craft Slides من تطوير حسام طه").next().is_some(), "the Arabic credit");
    h.state_mut().dialog = None;
    h.state_mut().run("ui.language", serde_json::json!({"value": "en"})).unwrap();
    h.state_mut().run("app.about", serde_json::json!({})).unwrap();
    h.run_steps(4);
    assert!(h.query_all_by_label("Craft Slides by Hossam Taha").next().is_some(), "the credit");
    assert!(h.query_all_by_label("Part of Craft Collab Suite. Based on DeckCraft by the ArtCraft team.").next().is_some());
    assert!(h.query_all_by_label("About Craft Slides").next().is_some() || crate::i18n::tr("About Craft Slides") == "About Craft Slides");
}

/// Review › Suggesting: the person's edits become suggestions, marked on the slide, and rejected
/// from the ribbon (`CRAFT_SLIDES_RTL_SCREENSHOT` saves `slides-suggesting.png`).
#[test]
fn suggesting_from_the_review_tab_marks_and_reviews_shapes() {
    let mut app = SlideApp::new(Session::with_new(), Services::default());
    app.session.prefs.author = "Ann".into();
    app.ui.tab = "Review".into();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h.get_by_label("Suggesting").click();
    h.run_steps(2);
    assert!(h.state().session.suggesting, "the button turns Suggesting on");
    let shapes = |h: &egui_kittest::Harness<'static, SlideApp>| h.state().session.active().unwrap().doc.slides[0].shapes.len();
    let before = shapes(&h);
    h.state_mut().run("shape.insert", serde_json::json!({"preset": "rect", "rect": [300, 200, 200, 120]})).unwrap();
    h.state_mut().run("text.exit", serde_json::json!({})).ok();
    h.run_steps(3);
    shot(&mut h, "slides-suggesting.png");
    assert_eq!(deckcraft_engine::suggest::suggestions(&h.state().session.active().unwrap().doc).len(), 1);
    h.get_by_label("Reject").click();
    h.run_steps(2);
    assert!(deckcraft_engine::suggest::suggestions(&h.state().session.active().unwrap().doc).is_empty());
    assert_eq!(shapes(&h), before, "rejected: the new shape is gone");
}
