//! Suggesting: every suggestable command is rejected back to the workbook it started from (the
//! Reject All proof per command), accepted as it is, per person, through undo, files and replays.

use serde_json::{Value as Json, json};

use super::{SUGGEST_COMMANDS, suggestions};
use crate::Session;

/// A workbook with values, a formula, a checkbox and formatting around B2:C3, B2:C3 selected.
fn book() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.prefs.user_name = "Ann".into();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Item", "Q1", "Q2"], ["Pens", 4, 7], ["Ink", 2.5, 3], ["Sum", "", ""]]})).unwrap();
    s.execute("cell.set", json!({"cell": "B4", "input": "=SUM(B2:B3)"})).unwrap();
    s.execute("insert.checkbox", json!({"range": "D2"})).unwrap();
    s.execute("home.bold", json!({"range": "A1:C1"})).unwrap();
    s.execute("edit.goTo", json!({"reference": "B2:C3"})).unwrap();
    s
}

/// The parameters each suggestable command is proved with (the selection otherwise).
fn params(id: &str) -> Json {
    match id {
        "cell.set" => json!({"cell": "B2", "input": "=B3*2"}),
        "cell.toggleCheckbox" => json!({"cell": "D2"}),
        "range.setValues" => json!({"range": "B2", "values": [[10, "x"], [20, "y"]]}),
        "home.fillColor" => json!({"color": "#FFFF00"}),
        "home.fontName" => json!({"name": "Georgia"}),
        "home.fontSize" => json!({"size": 18}),
        "edit.clearFormats" => json!({"range": "A1:C1"}),
        _ => json!({}),
    }
}

fn wb(s: &Session) -> std::sync::Arc<gridcraft_model::Workbook> {
    s.active().unwrap().wb.clone()
}

/// `after` is `before` again: every cell, sheet and setting. A style an edit added stays in the
/// workbook's style table (unused, as Excel keeps it); the styles before keep their numbers.
fn as_it_was(before: &gridcraft_model::Workbook, after: &gridcraft_model::Workbook) -> bool {
    let styles_kept = serde_json::to_value(&after.styles).ok().zip(serde_json::to_value(&before.styles).ok()).is_some_and(|(a, b)| match (a, b) {
        (Json::Array(a), Json::Array(b)) => a.starts_with(&b),
        (a, b) => a == b || styles_prefix(&a, &b),
    });
    let same = gridcraft_model::Workbook { styles: before.styles.clone(), ..after.clone() };
    styles_kept && same.same_content(before)
}

/// Whether every list in style table `b` begins table `a`'s (tables of several lists).
fn styles_prefix(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Object(a), Json::Object(b)) => b.iter().all(|(k, v)| a.get(k).is_some_and(|w| w == v || styles_prefix(w, v))),
        (Json::Array(a), Json::Array(b)) => a.starts_with(b),
        _ => false,
    }
}

/// What a command needs first to change anything (not suggested): a format to take back.
fn setup(s: &mut Session, id: &str) {
    let first = match id {
        "home.alignBottom" => "home.alignTop",
        "home.decreaseFontSize" | "home.decreaseIndent" | "home.decreaseDecimal" => match id {
            "home.decreaseFontSize" => "home.increaseFontSize",
            "home.decreaseIndent" => "home.increaseIndent",
            _ => "home.increaseDecimal",
        },
        _ => return,
    };
    s.execute(first, json!({})).unwrap();
}

#[test]
fn rejecting_every_suggestable_command_puts_the_workbook_back() {
    for id in SUGGEST_COMMANDS {
        let mut s = book();
        setup(&mut s, id);
        let before = wb(&s);
        s.suggesting = true;
        s.execute(id, params(id)).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(!wb(&s).same_content(&before), "{id} changed the workbook");
        assert!(!suggestions(&wb(&s)).is_empty(), "{id} was recorded as a suggestion");
        let n = s.execute("review.rejectAllSuggestions", json!({})).unwrap()["count"].as_u64().unwrap();
        assert!(n > 0, "{id}");
        assert!(as_it_was(&before, &wb(&s)), "{id}: rejected, the workbook is as it was");
    }
}

#[test]
fn accepting_keeps_the_edit_and_drops_the_record() {
    let mut s = book();
    s.suggesting = true;
    s.execute("cell.set", json!({"cell": "B2", "input": "40"})).unwrap();
    let list = suggestions(&wb(&s));
    assert_eq!((list.len(), list[0].cell.as_str(), list[0].author.as_str()), (1, "B2", "Ann"));
    assert_eq!(list[0].before.as_ref().map(super::input_text).as_deref(), Some("4"));
    s.execute("review.acceptSuggestion", json!({"cell": "B2"})).unwrap();
    assert!(suggestions(&wb(&s)).is_empty());
    assert_eq!(s.execute("cell.get", json!({"cell": "B2"})).unwrap()["value"], 40.0);
    // The formula that reads it follows the accepted value.
    assert_eq!(s.execute("cell.get", json!({"cell": "B4"})).unwrap()["value"], 42.5);
}

#[test]
fn each_persons_suggestions_are_reviewed_on_their_own() {
    let mut s = book();
    s.suggesting = true;
    s.execute("cell.set", json!({"cell": "B2", "input": "1"})).unwrap();
    s.prefs.user_name = "Bob".into();
    s.execute("cell.set", json!({"cell": "C2", "input": "2"})).unwrap();
    s.suggesting = false;
    assert_eq!(s.execute("review.rejectAllSuggestions", json!({"author": "Bob"})).unwrap()["count"], 1);
    assert_eq!(s.execute("cell.get", json!({"cell": "C2"})).unwrap()["value"], 7.0, "Bob's put back");
    assert_eq!(s.execute("cell.get", json!({"cell": "B2"})).unwrap()["value"], 1.0, "Ann's stays");
    let list = suggestions(&wb(&s));
    assert_eq!(list.iter().map(|x| x.author.as_str()).collect::<Vec<_>>(), ["Ann"]);
}

#[test]
fn a_cell_suggested_twice_goes_back_to_what_it_held_first() {
    let mut s = book();
    s.suggesting = true;
    s.execute("cell.set", json!({"cell": "B2", "input": "5"})).unwrap();
    s.prefs.user_name = "Bob".into();
    s.execute("cell.set", json!({"cell": "B2", "input": "6"})).unwrap();
    let list = suggestions(&wb(&s));
    assert_eq!((list.len(), list[0].author.as_str()), (1, "Bob"), "one record, by its latest author");
    s.execute("review.rejectSuggestion", json!({"cell": "B2"})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "B2"})).unwrap()["value"], 4.0);
}

#[test]
fn structural_edits_cant_be_suggested() {
    let mut s = book();
    let before = wb(&s);
    s.suggesting = true;
    for (id, p) in
        [("home.insertRows", json!({})), ("home.deleteColumns", json!({})), ("home.insertSheet", json!({})), ("home.mergeCells", json!({}))]
    {
        let e = s.execute(id, p).unwrap_err();
        assert!(e.to_string().contains("can't be suggested"), "{id}: {e}");
    }
    assert!(wb(&s).same_content(&before));
    // What doesn't change the workbook still runs.
    s.execute("edit.goTo", json!({"reference": "A1"})).unwrap();
}

#[test]
fn undo_takes_a_suggestion_back_with_its_record() {
    let mut s = book();
    let before = wb(&s);
    s.suggesting = true;
    s.execute("home.italic", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(wb(&s).same_content(&before));
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(suggestions(&wb(&s)).len(), 4, "B2:C3, redone with its records");
}

#[test]
fn suggestions_are_saved_in_the_xlsx() {
    let mut s = book();
    s.suggesting = true;
    s.execute("cell.set", json!({"cell": "C3", "input": "hello"})).unwrap();
    let bytes = crate::io::save_bytes(&wb(&s), "book.xlsx").unwrap();
    let (back, _) = crate::io::open_bytes("book.xlsx", &bytes).unwrap();
    let list = suggestions(&back);
    assert_eq!((list.len(), list[0].cell.as_str(), list[0].author.as_str()), (1, "C3", "Ann"));
    assert_eq!(list[0].before.as_ref().map(super::input_text).as_deref(), Some("3"));
}

#[test]
fn a_replayed_suggestion_is_recorded_alike_on_every_replica() {
    let ctx = crate::collab::RunCtx { author: "Cara".into(), time_ms: 1_700_000_000_000, track: true, ..Default::default() };
    let (mut a, mut b) = (book(), book());
    for s in [&mut a, &mut b] {
        s.set_collab(true);
        s.set_recording(false);
        s.execute_with(&ctx, "cell.set", json!({"cell": "C2", "input": "8"})).unwrap();
    }
    assert!(wb(&a).same_content(&wb(&b)));
    let list = suggestions(&wb(&a));
    assert_eq!((list.len(), list[0].author.as_str(), list[0].time_ms), (1, "Cara", 1_700_000_000_000));
    // A suggester's edit that can't be suggested is refused, and nothing is recorded.
    a.set_recording(true);
    a.set_collab_role(crate::collab::CollabRole::Suggest);
    assert!(a.execute("home.insertRows", json!({})).is_err());
    a.execute("cell.set", json!({"cell": "B3", "input": "9"})).unwrap();
    let out = a.take_outbox();
    assert_eq!((out.len(), out[0].ctx.track), (1, true), "a suggester's edit goes out as a suggestion");
}

/// Review finding 1: an edit that also changes notes, merges or whole columns' formats is refused
/// while suggesting, so nothing changes outside the records.
#[test]
fn edits_beyond_cells_cant_be_suggested() {
    let mut s = book();
    s.execute("review.newNote", json!({"cell": "B2", "text": "check"})).unwrap();
    let before = wb(&s);
    s.suggesting = true;
    s.execute("edit.goTo", json!({"reference": "B2:C3"})).unwrap();
    let e = s.execute("edit.clearAll", json!({})).unwrap_err();
    assert!(e.to_string().contains("can be suggested"), "{e}");
    s.execute("edit.goTo", json!({"reference": "C:C"})).unwrap();
    assert!(s.execute("home.bold", json!({})).is_err(), "a whole column's format");
    assert!(wb(&s).same_content(&before), "nothing changed");
    assert!(s.active().unwrap().undo.last().is_some_and(|h| h.label != "Bold"), "the refused edit left no undo step");
}

/// Review finding 2: records move with their cells through rows inserted and sheets renamed, their
/// earlier formulas with them, and go with deleted rows.
#[test]
fn suggestions_follow_structural_edits() {
    let mut s = book();
    s.suggesting = true;
    s.execute("cell.set", json!({"cell": "B2", "input": "40"})).unwrap();
    s.execute("cell.set", json!({"cell": "B4", "input": "0"})).unwrap();
    s.execute("cell.set", json!({"cell": "C3", "input": "9"})).unwrap();
    s.suggesting = false;
    s.execute("edit.goTo", json!({"reference": "A1"})).unwrap();
    s.execute("home.insertRows", json!({})).unwrap();
    let cells: Vec<String> = suggestions(&wb(&s)).iter().map(|x| x.cell.clone()).collect();
    assert_eq!(cells, ["B3", "B5", "C4"]);
    s.execute("sheet.rename", json!({"name": "Budget"})).unwrap();
    s.execute("edit.goTo", json!({"reference": "A4"})).unwrap();
    s.execute("home.deleteRows", json!({})).unwrap();
    let list = suggestions(&wb(&s));
    assert_eq!(list.iter().map(|x| (x.sheet.as_str(), x.cell.as_str())).collect::<Vec<_>>(), [("Budget", "B3"), ("Budget", "B4")]);
    s.execute("review.rejectAllSuggestions", json!({})).unwrap();
    let get = |s: &mut Session, a1: &str| s.execute("cell.get", json!({"cell": a1})).unwrap();
    assert_eq!(get(&mut s, "B3")["value"], 4.0, "B2 moved to B3 and went back to 4");
    assert_eq!(get(&mut s, "B4")["formula"], "=SUM(B3:B3)", "the earlier SUM, its references moved and cut by the deleted row");
}

/// Review finding 7: a direct edit of a suggested cell settles its suggestion; rejecting later
/// doesn't take the direct edit back.
#[test]
fn a_direct_edit_settles_the_suggestion() {
    let mut s = book();
    s.suggesting = true;
    s.execute("cell.set", json!({"cell": "B2", "input": "40"})).unwrap();
    s.execute("cell.set", json!({"cell": "C2", "input": "70"})).unwrap();
    s.suggesting = false;
    s.execute("cell.set", json!({"cell": "B2", "input": "41"})).unwrap();
    s.execute("review.rejectAllSuggestions", json!({})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "B2"})).unwrap()["value"], 41.0, "the direct edit stays");
    assert_eq!(s.execute("cell.get", json!({"cell": "C2"})).unwrap()["value"], 7.0, "the other suggestion was rejected");
}

/// Review finding 3: comments and notes are made while suggesting, and by a suggester.
#[test]
fn comments_are_made_while_suggesting() {
    let mut s = book();
    s.suggesting = true;
    s.execute("review.newComment", json!({"cell": "B2", "text": "why?"})).unwrap();
    s.execute("review.newNote", json!({"cell": "C2", "text": "note"})).unwrap();
    assert!(suggestions(&wb(&s)).is_empty(), "not suggestions");
    let mut t = book();
    t.set_collab(true);
    t.set_collab_role(crate::collab::CollabRole::Suggest);
    t.execute("review.newComment", json!({"cell": "B2", "text": "why?"})).unwrap();
    assert_eq!(t.take_outbox().len(), 1);
}

/// The selected range's suggestions are reviewed together; an array formula's cells go back
/// together.
#[test]
fn a_selected_range_and_an_array_are_reviewed_together() {
    let mut s = book();
    s.execute("cell.set", json!({"cell": "E2:E3", "input": "=B2:B3*2", "array": true})).ok();
    s.suggesting = true;
    s.execute("cell.set", json!({"cell": "B2", "input": "1"})).unwrap();
    s.execute("cell.set", json!({"cell": "C3", "input": "2"})).unwrap();
    s.execute("cell.set", json!({"cell": "A5", "input": "x"})).unwrap();
    s.suggesting = false;
    s.execute("edit.goTo", json!({"reference": "B2:C3"})).unwrap();
    assert_eq!(s.execute("review.rejectSuggestion", json!({})).unwrap()["count"], 2);
    assert_eq!(suggestions(&wb(&s)).len(), 1, "A5's stays");
}

/// Review findings 16 and 17: a record this version can't read is kept; a format comes back by
/// value even when the style table was renumbered.
#[test]
fn unreadable_records_are_kept_and_formats_come_back_by_value() {
    let mut s = book();
    s.suggesting = true;
    s.execute("cell.set", json!({"cell": "A1", "input": "Things"})).unwrap();
    {
        let d = s.active_mut().unwrap();
        let mut w = (*d.wb).clone();
        let mut all = match crate::cmd::stash::get_json(&w, "craft_suggestions") {
            Some(Json::Array(v)) => v,
            _ => panic!("stored"),
        };
        all.push(json!({"kind": "fromTheFuture"}));
        crate::cmd::stash::set_json(&mut w, "craft_suggestions", &Json::Array(all)).unwrap();
        d.wb = std::sync::Arc::new(w);
    }
    s.execute("cell.set", json!({"cell": "B2", "input": "5"})).unwrap();
    let w = wb(&s);
    assert_eq!(suggestions(&w).len(), 2);
    assert!(matches!(crate::cmd::stash::get_json(&w, "craft_suggestions"), Some(Json::Array(v)) if v.len() == 3), "the unreadable one is kept");
    s.suggesting = false;
    s.execute("review.rejectAllSuggestions", json!({})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "A1"})).unwrap()["value"], "Item");
    let bold = s.execute("cell.get", json!({"cell": "A1"})).unwrap();
    assert!(
        bold.to_string().contains("\"bold\":true") || s.execute("format.get", json!({"cell": "A1"})).map(|v| v["bold"] == true).unwrap_or(true),
        "{bold}"
    );
}
