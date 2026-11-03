//! Moving a person's selection through someone else's structural edit, as Excel for the web
//! does: cells below inserted rows move down, cells in deleted rows go to the first row after
//! them (the one now in their place), cut-and-pasted cells go with the block, a renamed sheet
//! keeps its people, and the people on a deleted sheet go to the sheet now in its place.

use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS, RangeRef};
use gridcraft_engine::collab::SheetPlace;
use gridcraft_formula::adjust::{Axis, Edit};
use gridcraft_model::structure::Structural;

/// `v` after `count` lines were inserted before `at`.
fn inserted(v: u32, at: u32, count: u32, limit: u32) -> u32 {
    if v >= at { v.saturating_add(count).min(limit - 1) } else { v }
}

/// `v` after the `count` lines from `at` were deleted.
fn deleted(v: u32, at: u32, count: u32) -> u32 {
    if v >= at.saturating_add(count) {
        v - count
    } else if v >= at {
        at
    } else {
        v
    }
}

/// A cell after an edit on its own sheet.
pub fn map_cell(c: CellRef, e: &Edit) -> CellRef {
    match *e {
        Edit::Insert { axis: Axis::Rows, at, count } => CellRef { row: inserted(c.row, at, count, MAX_ROWS), ..c },
        Edit::Insert { axis: Axis::Cols, at, count } => CellRef { col: inserted(c.col, at, count, MAX_COLS), ..c },
        Edit::Delete { axis: Axis::Rows, at, count } => CellRef { row: deleted(c.row, at, count), ..c },
        Edit::Delete { axis: Axis::Cols, at, count } => CellRef { col: deleted(c.col, at, count), ..c },
        Edit::Move { from, to_row, to_col } | Edit::MoveToSheet { from, to_row, to_col, .. } if from.contains(c) => moved(c, from, to_row, to_col),
        Edit::Move { .. } | Edit::MoveToSheet { .. } => c,
        // Insert Cells shifting down (right): the cells in the range's columns (rows), from its
        // first row (column) on, move by its height (width); Delete Cells moves them back, and
        // the deleted ones go to the first cell after.
        Edit::InsertCells { axis: Axis::Rows, range } if in_cols(c, range) => {
            CellRef { row: inserted(c.row, range.start.row, height(range), MAX_ROWS), ..c }
        }
        Edit::InsertCells { axis: Axis::Cols, range } if in_rows(c, range) => {
            CellRef { col: inserted(c.col, range.start.col, width(range), MAX_COLS), ..c }
        }
        Edit::DeleteCells { axis: Axis::Rows, range } if in_cols(c, range) => CellRef { row: deleted(c.row, range.start.row, height(range)), ..c },
        Edit::DeleteCells { axis: Axis::Cols, range } if in_rows(c, range) => CellRef { col: deleted(c.col, range.start.col, width(range)), ..c },
        Edit::InsertCells { .. } | Edit::DeleteCells { .. } => c,
    }
}

fn in_cols(c: CellRef, r: RangeRef) -> bool {
    c.col >= r.start.col && c.col <= r.end.col
}

fn in_rows(c: CellRef, r: RangeRef) -> bool {
    c.row >= r.start.row && c.row <= r.end.row
}

fn height(r: RangeRef) -> u32 {
    r.end.row - r.start.row + 1
}

fn width(r: RangeRef) -> u32 {
    r.end.col - r.start.col + 1
}

fn moved(c: CellRef, from: RangeRef, to_row: u32, to_col: u32) -> CellRef {
    let row = (c.row - from.start.row).saturating_add(to_row).min(MAX_ROWS - 1);
    let col = (c.col - from.start.col).saturating_add(to_col).min(MAX_COLS - 1);
    CellRef { row, col }
}

fn map_range(r: RangeRef, e: &Edit) -> RangeRef {
    let (a, b) = (map_cell(r.start, e), map_cell(r.end, e));
    RangeRef { start: CellRef { row: a.row.min(b.row), col: a.col.min(b.col) }, end: CellRef { row: a.row.max(b.row), col: a.col.max(b.col) } }
}

/// A selection after one structural edit (anyone's).
pub fn map_place(p: &SheetPlace, e: &Structural) -> SheetPlace {
    let mut out = p.clone();
    match e {
        Structural::Edit { sheet, edit } if *sheet == p.sheet => {
            if let Edit::MoveToSheet { from, to_sheet, .. } = edit
                && from.contains(p.selection.active)
            {
                out.sheet = to_sheet.clone();
            }
            let s = &mut out.selection;
            s.active = map_cell(s.active, edit);
            s.anchor = map_cell(s.anchor, edit);
            for r in &mut s.ranges {
                *r = map_range(*r, edit);
            }
        }
        Structural::Renamed { from, to } if *from == p.sheet => out.sheet = to.clone(),
        Structural::Removed { name, next, .. } if *name == p.sheet => out.sheet = next.clone(),
        _ => {}
    }
    out
}

/// Cells on sheet `sheet` after several structural edits, in order (a rename or a move to another
/// sheet follows them there).
pub fn map_range_through(sheet: &str, r: RangeRef, edits: &[Structural]) -> RangeRef {
    let mut sheet = sheet.to_string();
    let mut r = r;
    for e in edits {
        match e {
            Structural::Edit { sheet: s, edit } if *s == sheet => {
                if let Edit::MoveToSheet { from, to_sheet, .. } = edit
                    && from.contains(r.start)
                {
                    sheet = to_sheet.clone();
                }
                r = map_range(r, edit);
            }
            Structural::Renamed { from, to } if *from == sheet => sheet = to.clone(),
            _ => {}
        }
    }
    r
}

/// A selection after several structural edits, in order.
pub fn map_through(p: &SheetPlace, edits: &[Structural]) -> SheetPlace {
    edits.iter().fold(p.clone(), |p, e| map_place(&p, e))
}

#[cfg(test)]
mod tests {
    use gridcraft_engine::Selection;

    use super::*;

    fn at(a1: &str) -> SheetPlace {
        SheetPlace { sheet: "Sheet1".into(), selection: Selection::at(CellRef::parse(a1).unwrap()) }
    }

    fn on(sheet: &str, edit: Edit) -> Structural {
        Structural::Edit { sheet: sheet.into(), edit }
    }

    fn cell(p: &SheetPlace) -> String {
        p.selection.active.a1()
    }

    #[test]
    fn cells_below_inserted_rows_move_down_and_those_above_stay() {
        let e = on("Sheet1", Edit::Insert { axis: Axis::Rows, at: 2, count: 2 });
        assert_eq!(cell(&map_place(&at("B5"), &e)), "B7");
        assert_eq!(cell(&map_place(&at("B3"), &e)), "B5", "row 3 is index 2: it moves");
        assert_eq!(cell(&map_place(&at("B2"), &e)), "B2");
    }

    #[test]
    fn cells_in_deleted_rows_go_to_the_row_now_in_their_place() {
        let e = on("Sheet1", Edit::Delete { axis: Axis::Rows, at: 2, count: 3 });
        assert_eq!(cell(&map_place(&at("C4"), &e)), "C3", "inside the band");
        assert_eq!(cell(&map_place(&at("C9"), &e)), "C6", "below it");
        assert_eq!(cell(&map_place(&at("C1"), &e)), "C1", "above it");
    }

    #[test]
    fn deleting_the_last_rows_keeps_positions_on_the_sheet() {
        let e = on("Sheet1", Edit::Delete { axis: Axis::Rows, at: MAX_ROWS - 2, count: 2 });
        let p = SheetPlace { sheet: "Sheet1".into(), selection: Selection::at(CellRef { row: MAX_ROWS - 1, col: 0 }) };
        assert!(map_place(&p, &e).selection.active.row < MAX_ROWS);
        let e = on("Sheet1", Edit::Insert { axis: Axis::Rows, at: 0, count: 5 });
        assert_eq!(map_place(&p, &e).selection.active.row, MAX_ROWS - 1, "pushed rows stop at the last row");
    }

    #[test]
    fn columns_move_as_rows_do() {
        let e = on("Sheet1", Edit::Insert { axis: Axis::Cols, at: 1, count: 1 });
        assert_eq!(cell(&map_place(&at("B2"), &e)), "C2");
        let e = on("Sheet1", Edit::Delete { axis: Axis::Cols, at: 0, count: 1 });
        assert_eq!(cell(&map_place(&at("C2"), &e)), "B2");
    }

    #[test]
    fn an_edit_on_another_sheet_moves_nothing() {
        let e = on("Sheet2", Edit::Insert { axis: Axis::Rows, at: 0, count: 10 });
        assert_eq!(cell(&map_place(&at("A1"), &e)), "A1");
    }

    #[test]
    fn cut_and_pasted_cells_take_their_people_with_them() {
        let from = RangeRef::parse("A1:B2").unwrap();
        let e = on("Sheet1", Edit::Move { from, to_row: 9, to_col: 3 });
        assert_eq!(cell(&map_place(&at("B2"), &e)), "E11");
        assert_eq!(cell(&map_place(&at("C3"), &e)), "C3");
        let e = on("Sheet1", Edit::MoveToSheet { from, to_sheet: "Data".into(), to_row: 0, to_col: 0 });
        let p = map_place(&at("A2"), &e);
        assert_eq!((p.sheet.as_str(), cell(&p).as_str()), ("Data", "A2"));
    }

    #[test]
    fn insert_and_delete_cells_shift_only_their_band() {
        let range = RangeRef::parse("B2:C3").unwrap();
        let e = on("Sheet1", Edit::InsertCells { axis: Axis::Rows, range });
        assert_eq!(cell(&map_place(&at("B5"), &e)), "B7", "in the columns, below: down by the height");
        assert_eq!(cell(&map_place(&at("D5"), &e)), "D5", "another column");
        let e = on("Sheet1", Edit::DeleteCells { axis: Axis::Cols, range });
        assert_eq!(cell(&map_place(&at("E2"), &e)), "C2", "in the rows, right of it: left by the width");
        assert_eq!(cell(&map_place(&at("C3"), &e)), "B3", "inside it: to its first column");
    }

    #[test]
    fn renamed_and_deleted_sheets() {
        let p = map_place(&at("A1"), &Structural::Renamed { from: "Sheet1".into(), to: "Data".into() });
        assert_eq!(p.sheet, "Data");
        let p = map_place(&p, &Structural::Removed { name: "Data".into(), index: 0, next: "Sheet2".into() });
        assert_eq!(p.sheet, "Sheet2");
    }

    #[test]
    fn a_range_stays_a_range() {
        let mut p = at("A1");
        p.selection.ranges = vec![RangeRef::parse("A2:A6").unwrap()];
        let p = map_place(&p, &on("Sheet1", Edit::Delete { axis: Axis::Rows, at: 0, count: 3 }));
        assert_eq!(p.selection.ranges[0], RangeRef::parse("A1:A3").unwrap());
    }
}
