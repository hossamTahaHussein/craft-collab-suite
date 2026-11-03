//! The workbook's structural log (`gridcraft_model::structure`): every command that shifts
//! rows, columns or cells, renames or deletes a sheet leaves its edits there.

use gridcraft_formula::adjust::{Axis, Edit};
use gridcraft_model::structure::Structural;
use serde_json::json;

use crate::Session;

fn s() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}

fn after(s: &mut Session, cmd: &str, p: serde_json::Value) -> Vec<Structural> {
    let before = s.doc().unwrap().wb.structure.clone();
    s.execute(cmd, p).unwrap();
    s.doc().unwrap().wb.structure.since(&before).unwrap()
}

#[test]
fn inserting_rows_logs_the_insert() {
    let mut s = s();
    let log = after(&mut s, "home.insertRows", json!({"rows": "3:4"}));
    assert_eq!(log, [Structural::Edit { sheet: "Sheet1".into(), edit: Edit::Insert { axis: Axis::Rows, at: 2, count: 2 } }]);
}

#[test]
fn deleting_columns_logs_the_delete() {
    let mut s = s();
    let log = after(&mut s, "home.deleteColumns", json!({"cols": "B:B"}));
    assert_eq!(log, [Structural::Edit { sheet: "Sheet1".into(), edit: Edit::Delete { axis: Axis::Cols, at: 1, count: 1 } }]);
}

#[test]
fn renaming_and_deleting_sheets_are_logged() {
    let mut s = s();
    s.execute("home.insertSheet", json!({})).unwrap();
    let log = after(&mut s, "sheet.rename", json!({"sheet": 0, "name": "Data"}));
    assert_eq!(log, [Structural::Renamed { from: "Sheet1".into(), to: "Data".into() }]);
    let log = after(&mut s, "home.deleteSheet", json!({"sheet": 0}));
    assert_eq!(log, [Structural::Removed { name: "Data".into(), index: 0 }]);
}

#[test]
fn a_cell_edit_logs_nothing() {
    let mut s = s();
    assert!(after(&mut s, "cell.set", json!({"cell": "A1", "input": "x"})).is_empty());
}
