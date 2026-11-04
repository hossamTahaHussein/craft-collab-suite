//! The Arabic interface reads from the right: the direction follows the interface language,
//! and the chrome mirrors (as in Craft Docs). `CRAFT_SHEETS_RTL_SCREENSHOT=path.png` saves the window.

use egui_kittest::kittest::Queryable;
use gridcraft_ui_egui::i18n::Language;
use serde_json::json;

use crate::support::{app_harness, blank_session};

fn arabic() -> crate::common::Harness {
    let mut app = crate::common::app(blank_session());
    app.ui.language = Language::Ar;
    app_harness(app, 6)
}

/// The rect of the first node labelled `label`.
fn at(h: &crate::common::Harness, label: &str) -> egui::Rect {
    h.query_by_label(label).unwrap_or_else(|| panic!("{label} is drawn")).rect()
}

#[test]
fn arabic_is_a_language_and_reads_right_to_left() {
    assert_eq!(Language::parse("ar-EG"), Some(Language::Ar));
    assert!(Language::Ar.is_rtl() && !Language::En.is_rtl());
    assert_eq!(Language::Ar.tr("Paste"), "لصق");
    assert_eq!(Language::Ar.tr("Version history"), "سجل الإصدارات");
    assert!(Language::for_picker().contains(&Language::Ar));
}

#[test]
fn the_ribbon_tabs_start_at_the_right_in_arabic() {
    let mut h = arabic();
    assert!(craft_ui::direction::is_rtl(&h.ctx), "the direction follows the language");
    if let Ok(path) = std::env::var("CRAFT_SHEETS_RTL_SCREENSHOT") {
        h.render().unwrap().save(path).unwrap();
    }
    let home = at(&h, "الصفحة الرئيسية");
    let insert = at(&h, "إدراج");
    assert!(home.center().x > insert.center().x, "Home before Insert, reading from the right: {home:?} {insert:?}");
    // Back to English: left to right again.
    h.state_mut().run("app.language.set", json!({"language": "en"})).unwrap();
    h.run_steps(4);
    assert!(!craft_ui::direction::is_rtl(&h.ctx));
    assert!(at(&h, "Home").center().x < at(&h, "Insert").center().x);
}

/// As Excel's Default direction: a new blank workbook's sheets read right to left in Arabic (and
/// the workbook still counts as unchanged), and a sheet inserted is right to left too.
#[test]
fn new_sheets_read_right_to_left_in_arabic() {
    let mut h = arabic();
    let rtl = |h: &crate::common::Harness| h.state().session.active().unwrap().wb.sheets.iter().map(|s| s.right_to_left).collect::<Vec<_>>();
    assert_eq!(rtl(&h), [true]);
    assert!(!h.state().session.active().unwrap().is_dirty(), "a blank workbook stays unchanged");
    h.state_mut().run("home.insertSheet", json!({})).unwrap();
    assert_eq!(rtl(&h), [true, true]);
    h.state_mut().run("app.language.set", json!({"language": "en"})).unwrap();
    h.state_mut().run("file.new", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(rtl(&h), [false], "an English interface's new workbook reads left to right");
}

/// The File page and a dialog in Arabic (screenshots with `CRAFT_SHEETS_RTL_SCREENSHOT`).
#[test]
fn the_file_page_and_dialogs_read_from_the_right() {
    let mut h = arabic();
    h.state_mut().open_backstage(gridcraft_ui_egui::backstage::Page::Home);
    h.run_steps(4);
    let back = at(&h, "رجوع");
    assert!(back.center().x > 600.0, "the File page's menu is at the right: {back:?}");
    if let Ok(path) = std::env::var("CRAFT_SHEETS_RTL_SCREENSHOT") {
        h.render().unwrap().save(std::path::Path::new(&path).with_file_name("sheets-ar-file.png")).unwrap();
    }
    h.state_mut().backstage = None;
    h.state_mut().open_dialog("formatCells", json!({}));
    h.run_steps(6);
    if let Ok(path) = std::env::var("CRAFT_SHEETS_RTL_SCREENSHOT") {
        h.render().unwrap().save(std::path::Path::new(&path).with_file_name("sheets-ar-dialog.png")).unwrap();
    }
}

/// Justified cells stretch every line but the last of a paragraph: Arabic with kashida, English
/// by its word gaps (`CRAFT_SHEETS_RTL_SCREENSHOT` saves `sheets-justify.png`).
#[test]
fn justified_cells_stretch_arabic_with_kashida_and_english_by_its_gaps() {
    let mut app = crate::common::app(blank_session());
    app.ui.language = Language::En;
    let mut h = app_harness(app, 4);
    let arabic = "مرحبا بكم في جدول البيانات الجديد هذا نص طويل يمتد على عدة أسطر";
    let english = "Justified text in a cell spreads its words to both edges of the column";
    h.state_mut().run("range.setValues", json!({"range": "B2", "values": [[arabic, english]]})).unwrap();
    h.state_mut().run("home.justify", json!({"range": "B2:C2"})).unwrap();
    h.state_mut().run("home.columnWidth", json!({"cols": "B:C", "width": 180})).unwrap();
    h.state_mut().run("home.rowHeight", json!({"rows": "2:2", "height": 110})).unwrap();
    h.run_steps(4);
    if let Ok(path) = std::env::var("CRAFT_SHEETS_RTL_SCREENSHOT") {
        h.render().unwrap().save(std::path::Path::new(&path).with_file_name("sheets-justify.png")).unwrap();
    }
}

/// About names the suite and credits its author, in the interface language (Arabic credit read
/// right to left).
#[test]
fn about_credits_hossam_taha_in_the_interface_language() {
    let mut app = crate::common::app(blank_session());
    app.ui.language = Language::En;
    let mut h = app_harness(app, 4);
    h.state_mut().open_dialog("about", json!({}));
    h.run_steps(4);
    assert!(h.query_by_label("Craft Sheets by Hossam Taha").is_some(), "the credit");
    assert!(h.query_by_label("Part of Craft Collab Suite.").is_some());
    h.state_mut().dialog = None;
    h.state_mut().run("app.language.set", json!({"language": "ar"})).unwrap();
    h.state_mut().open_dialog("about", json!({}));
    h.run_steps(4);
    assert!(h.query_by_label("Craft Sheets من تطوير حسام طه").is_some(), "the Arabic credit");
    assert!(h.query_by_label("حول").is_some(), "the tabs are translated");
}

/// Review › Suggesting: the person's edits become suggestions, marked in the cell, accepted or
/// rejected from the ribbon (`CRAFT_SHEETS_RTL_SCREENSHOT` saves `sheets-suggesting.png`).
#[test]
fn suggesting_from_the_review_tab_marks_and_reviews_cells() {
    let mut app = crate::common::app(blank_session());
    app.ui.language = Language::En;
    let mut h = app_harness(app, 4);
    h.state_mut().run("range.setValues", json!({"range": "B2", "values": [[4, 7]]})).unwrap();
    h.state_mut().run("ui.ribbonTab", json!({"tab": "Review"})).unwrap();
    h.run_steps(3);
    h.get_by_label("Suggesting").click();
    h.run_steps(2);
    assert!(h.state().session.suggesting, "the button turns Suggesting on");
    h.state_mut().run("cell.set", json!({"cell": "B2", "input": "40"})).unwrap();
    h.state_mut().run("edit.goTo", json!({"reference": "B2"})).unwrap();
    h.state_mut().grid.hover_cell = gridcraft_engine::core::CellRef::parse("B2");
    h.run_steps(3);
    if let Ok(path) = std::env::var("CRAFT_SHEETS_RTL_SCREENSHOT") {
        h.render().unwrap().save(std::path::Path::new(&path).with_file_name("sheets-suggesting.png")).unwrap();
    }
    assert!(h.query_by_label_contains("suggested this").is_some(), "the card says who suggested it");
    h.get_by_label("Reject").click();
    h.run_steps(2);
    let d = h.state().session.active().unwrap();
    assert!(gridcraft_engine::suggest::suggestions(&d.wb).is_empty());
    assert_eq!(h.state_mut().run("cell.get", json!({"cell": "B2"})).unwrap()["value"], 4.0, "rejected: back to 4");
}

/// README pictures (`CRAFT_README_SHOTS=dir` writes `sheets.png` and `sheets-arabic.png`): a
/// budget co-edited by three people with a suggestion shown, and the same in Arabic.
#[test]
fn readme_pictures() {
    let Ok(dir) = std::env::var("CRAFT_README_SHOTS") else { return };
    for (lang, file, rows) in [
        (
            Language::En,
            "sheets.png",
            json!([
                ["Team budget", "Q1", "Q2", "Q3", "Q4"],
                ["Travel", 1200, 980, 1430, 1100],
                ["Software", 640, 640, 720, 720],
                ["Events", 2300, 0, 1850, 400],
                ["Training", 450, 900, 300, 650]
            ]),
        ),
        (
            Language::Ar,
            "sheets-arabic.png",
            json!([
                ["ميزانية الفريق", "الربع 1", "الربع 2", "الربع 3", "الربع 4"],
                ["السفر", 1200, 980, 1430, 1100],
                ["البرامج", 640, 640, 720, 720],
                ["الفعاليات", 2300, 0, 1850, 400],
                ["التدريب", 450, 900, 300, 650]
            ]),
        ),
    ] {
        let mut app = crate::common::app(blank_session());
        app.ui.language = lang;
        let mut h = app_harness(app, 4);
        let s = h.state_mut();
        s.run("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
        s.run("cell.set", json!({"cell": "A7", "input": if lang == Language::Ar { "المجموع" } else { "Total" }})).unwrap();
        for col in ["B", "C", "D", "E"] {
            s.run("cell.set", json!({"cell": format!("{col}7"), "input": format!("=SUM({col}2:{col}5)")})).unwrap();
        }
        s.run("home.bold", json!({"range": "A1:E1"})).unwrap();
        s.run("home.bold", json!({"range": "A7:E7"})).unwrap();
        s.run("home.columnWidth", json!({"cols": "A:A", "width": 130})).unwrap();
        s.run("home.columnWidth", json!({"cols": "B:E", "width": 90})).unwrap();
        s.session.prefs.user_name = if lang == Language::Ar { "سارة حسن".into() } else { "Sara Hassan".into() };
        s.run("review.suggesting", json!({"on": true})).unwrap();
        s.run("cell.set", json!({"cell": "C4", "input": "1500"})).unwrap();
        s.run("review.suggesting", json!({"on": false})).unwrap();
        s.run("edit.goTo", json!({"reference": "B3"})).unwrap();
        let place = |a1: &str| gridcraft_engine::collab::SheetPlace {
            sheet: "Sheet1".into(),
            selection: gridcraft_engine::Selection::at(gridcraft_engine::core::CellRef::parse(a1).unwrap()),
        };
        s.peers = vec![
            gridcraft_ui_egui::grid::peers::RemotePeer {
                name: if lang == Language::Ar { "عمر".into() } else { "Bob Diaz".into() },
                color: 1,
                place: Some(place("D5")),
                typing: false,
            },
            gridcraft_ui_egui::grid::peers::RemotePeer {
                name: if lang == Language::Ar { "ليلى".into() } else { "Cara Ng".into() },
                color: 2,
                place: Some(place("E3")),
                typing: true,
            },
        ];
        s.grid.hover_cell = gridcraft_engine::core::CellRef::parse("C4");
        h.run_steps(6);
        h.render().unwrap().save(std::path::Path::new(&dir).join(file)).unwrap();
    }
}
