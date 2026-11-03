//! A revision's structural edits as history entries carry them in snapshots:
//! `{"rev": 12, "edits": [{"k": "insert", "sheet": "Sheet1", "axis": "rows", "at": 2, "count": 1}]}`.

use gridcraft_core::RangeRef;
use gridcraft_formula::adjust::{Axis, Edit};
use gridcraft_model::structure::Structural;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Edits {
    pub edits: Vec<WireEdit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WireAxis {
    Rows,
    Cols,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WireEdit {
    Insert { sheet: String, axis: WireAxis, at: u32, count: u32 },
    Delete { sheet: String, axis: WireAxis, at: u32, count: u32 },
    Move { sheet: String, from: RangeRef, to_row: u32, to_col: u32 },
    InsertCells { sheet: String, axis: WireAxis, range: RangeRef },
    DeleteCells { sheet: String, axis: WireAxis, range: RangeRef },
    MoveToSheet { sheet: String, from: RangeRef, to_sheet: String, to_row: u32, to_col: u32 },
    Renamed { from: String, to: String },
    Removed { name: String, index: usize, next: String },
}

fn wa(a: Axis) -> WireAxis {
    match a {
        Axis::Rows => WireAxis::Rows,
        Axis::Cols => WireAxis::Cols,
    }
}

fn ax(a: WireAxis) -> Axis {
    match a {
        WireAxis::Rows => Axis::Rows,
        WireAxis::Cols => Axis::Cols,
    }
}

impl From<&Structural> for WireEdit {
    fn from(s: &Structural) -> WireEdit {
        match s {
            Structural::Edit { sheet, edit } => {
                let sheet = sheet.clone();
                match edit.clone() {
                    Edit::Insert { axis, at, count } => WireEdit::Insert { sheet, axis: wa(axis), at, count },
                    Edit::Delete { axis, at, count } => WireEdit::Delete { sheet, axis: wa(axis), at, count },
                    Edit::Move { from, to_row, to_col } => WireEdit::Move { sheet, from, to_row, to_col },
                    Edit::InsertCells { axis, range } => WireEdit::InsertCells { sheet, axis: wa(axis), range },
                    Edit::DeleteCells { axis, range } => WireEdit::DeleteCells { sheet, axis: wa(axis), range },
                    Edit::MoveToSheet { from, to_sheet, to_row, to_col } => WireEdit::MoveToSheet { sheet, from, to_sheet, to_row, to_col },
                }
            }
            Structural::Renamed { from, to } => WireEdit::Renamed { from: from.clone(), to: to.clone() },
            Structural::Removed { name, index, next } => WireEdit::Removed { name: name.clone(), index: *index, next: next.clone() },
        }
    }
}

impl From<WireEdit> for Structural {
    fn from(w: WireEdit) -> Structural {
        let on = |sheet: String, edit: Edit| Structural::Edit { sheet, edit };
        match w {
            WireEdit::Insert { sheet, axis, at, count } => on(sheet, Edit::Insert { axis: ax(axis), at, count }),
            WireEdit::Delete { sheet, axis, at, count } => on(sheet, Edit::Delete { axis: ax(axis), at, count }),
            WireEdit::Move { sheet, from, to_row, to_col } => on(sheet, Edit::Move { from, to_row, to_col }),
            WireEdit::InsertCells { sheet, axis, range } => on(sheet, Edit::InsertCells { axis: ax(axis), range }),
            WireEdit::DeleteCells { sheet, axis, range } => on(sheet, Edit::DeleteCells { axis: ax(axis), range }),
            WireEdit::MoveToSheet { sheet, from, to_sheet, to_row, to_col } => on(sheet, Edit::MoveToSheet { from, to_sheet, to_row, to_col }),
            WireEdit::Renamed { from, to } => Structural::Renamed { from, to },
            WireEdit::Removed { name, index, next } => Structural::Removed { name, index, next },
        }
    }
}

impl From<&[Structural]> for Edits {
    fn from(s: &[Structural]) -> Edits {
        Edits { edits: s.iter().map(WireEdit::from).collect() }
    }
}

impl From<Edits> for Vec<Structural> {
    fn from(e: Edits) -> Vec<Structural> {
        e.edits.into_iter().map(Structural::from).collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn edits_keep_their_wire_shape_and_read_back() {
        let edits = vec![
            Structural::Edit { sheet: "Sheet1".into(), edit: Edit::Insert { axis: Axis::Rows, at: 2, count: 1 } },
            Structural::Edit { sheet: "Sheet1".into(), edit: Edit::Move { from: RangeRef::parse("A1:B2").unwrap(), to_row: 4, to_col: 0 } },
            Structural::Renamed { from: "Sheet1".into(), to: "Data".into() },
            Structural::Removed { name: "Data".into(), index: 0, next: "Sheet2".into() },
        ];
        let wire = Edits::from(edits.as_slice());
        let v = serde_json::to_value(&wire).unwrap();
        assert_eq!(v["edits"][0], json!({"k": "insert", "sheet": "Sheet1", "axis": "rows", "at": 2, "count": 1}));
        assert_eq!(v["edits"][1]["k"], "move");
        assert_eq!(v["edits"][1]["toRow"], 4);
        let back: Edits = serde_json::from_value(v).unwrap();
        assert_eq!(Vec::<Structural>::from(back), edits);
    }
}
