//! The structural edits made to a workbook while it is open: rows and columns inserted, deleted
//! or moved, cells shifted, sheets renamed or removed. Co-editing moves people's positions
//! through them (a cell reference doesn't say which rows came and went between two versions).
//!
//! The log is a shared list: every version of a workbook holds the log as it was when that
//! version was made, and the newer versions share the older entries, so keeping versions (undo,
//! a co-editing replica's confirmed and visible workbooks) costs one node per edit. It is not
//! part of the workbook's content: not in JSON or the file, and two workbooks are equal whatever
//! their logs say.

use std::sync::Arc;

use gridcraft_formula::adjust::Edit;

/// One structural edit, naming the sheet it was made on.
#[derive(Clone, Debug, PartialEq)]
pub enum Structural {
    /// Rows, columns or cells shifted on sheet `sheet` (as formulas are rewritten for them).
    Edit {
        sheet: String,
        edit: Edit,
    },
    Renamed {
        from: String,
        to: String,
    },
    /// Sheet `name`, which was at `index`, was deleted; `next` is the sheet now in its place
    /// (or the one before it, for the last sheet), where positions on it go.
    Removed {
        name: String,
        index: usize,
        next: String,
    },
}

struct Node {
    entry: Structural,
    prev: Option<Arc<Node>>,
    len: usize,
}

#[derive(Clone, Default)]
pub struct StructureLog(Option<Arc<Node>>);

impl StructureLog {
    pub fn push(&mut self, entry: Structural) {
        let len = self.len() + 1;
        self.0 = Some(Arc::new(Node { entry, prev: self.0.take(), len }));
    }

    pub fn len(&self) -> usize {
        self.0.as_ref().map_or(0, |n| n.len)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// The edits made after `older`, oldest first, when this log continues `older`'s; `None`
    /// when it doesn't (another workbook, or one read again from a snapshot).
    pub fn since(&self, older: &StructureLog) -> Option<Vec<Structural>> {
        let mut out = Vec::new();
        let mut at = self.0.as_ref();
        while at.map(|n| n.len) > older.0.as_ref().map(|n| n.len) {
            let n = at?;
            out.push(n.entry.clone());
            at = n.prev.as_ref();
        }
        let same = match (at, older.0.as_ref()) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        same.then(|| {
            out.reverse();
            out
        })
    }
}

/// Not part of the workbook's content.
impl PartialEq for StructureLog {
    fn eq(&self, _other: &StructureLog) -> bool {
        true
    }
}

impl std::fmt::Debug for StructureLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StructureLog({} edits)", self.len())
    }
}

#[cfg(test)]
mod tests {
    use gridcraft_formula::adjust::Axis;

    use super::*;

    fn rows(at: u32) -> Structural {
        Structural::Edit { sheet: "Sheet1".into(), edit: Edit::Insert { axis: Axis::Rows, at, count: 1 } }
    }

    #[test]
    fn a_later_version_says_what_came_after_an_earlier_one() {
        let mut a = StructureLog::default();
        a.push(rows(1));
        let mut b = a.clone();
        b.push(rows(2));
        b.push(Structural::Renamed { from: "Sheet1".into(), to: "Data".into() });
        assert_eq!(b.since(&a), Some(vec![rows(2), Structural::Renamed { from: "Sheet1".into(), to: "Data".into() }]));
        assert_eq!(a.since(&a), Some(vec![]));
        assert_eq!(b.since(&StructureLog::default()).map(|v| v.len()), Some(3));
    }

    #[test]
    fn unrelated_logs_say_nothing() {
        let mut a = StructureLog::default();
        a.push(rows(1));
        let mut b = StructureLog::default();
        b.push(rows(1));
        b.push(rows(2));
        assert_eq!(b.since(&a), None, "same length prefix, different history");
        assert_eq!(a.since(&b), None, "older than the other");
    }

    #[test]
    fn the_log_is_not_content() {
        let mut a = StructureLog::default();
        a.push(rows(1));
        assert_eq!(a, StructureLog::default());
    }
}
