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
