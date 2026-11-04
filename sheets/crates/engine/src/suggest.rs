//! Suggesting: edits made as suggestions, which their author or anyone who may edit accepts or
//! rejects, cell by cell, person by person or all at once (as Craft Docs' Suggesting mode).
//!
//! A suggested edit shows at once, as typed; what each cell held before is kept with who
//! suggested it and when. Accepting keeps the cell as it is and drops the record; rejecting puts
//! back what the cell held. The records are part of the workbook (a hidden item, see
//! [`crate::cmd::stash`]), so they travel in its .xlsx, in co-editing snapshots and in undo
//! steps, and every replica records the same ones: an edit replayed with `track` on its context
//! records them as it runs.
//!
//! Only edits of cells' contents and formatting can be suggested ([`SUGGEST_COMMANDS`]): rejecting
//! one of those puts the workbook back exactly (one Reject All test per command proves it).
//! Structural edits (rows, columns, sheets) and the rest are refused while suggesting.

use std::sync::Arc;

use gridcraft_core::CellRef;
use gridcraft_model::{Cell, Formula, Workbook};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::cmd::{CommandSpec, bool_param, cmd, edit, has_doc, stash, str_param};
use crate::{EngineError, Result, Session};

/// The workbook item holding the suggestions.
const KEY: &str = "craft_suggestions";

/// The commands an edit may be suggested with: they change cells' contents and formatting only.
/// The relay's `suggest-commands.sheets.json` lists the same.
pub const SUGGEST_COMMANDS: &[&str] = &[
    "cell.set",
    "cell.toggleCheckbox",
    "range.setValues",
    "edit.clearAll",
    "edit.clearContents",
    "edit.clearFormats",
    "edit.fillDown",
    "edit.fillUp",
    "edit.fillLeft",
    "edit.fillRight",
    "home.accounting",
    "home.alignBottom",
    "home.alignCenter",
    "home.alignLeft",
    "home.alignMiddle",
    "home.alignRight",
    "home.alignTop",
    "home.bold",
    "home.comma",
    "home.decreaseDecimal",
    "home.increaseDecimal",
    "home.decreaseFontSize",
    "home.increaseFontSize",
    "home.decreaseIndent",
    "home.increaseIndent",
    "home.doubleUnderline",
    "home.fillColor",
    "home.fontName",
    "home.fontSize",
    "home.italic",
    "home.justify",
    "home.percent",
    "home.shrinkToFit",
    "home.strikethrough",
    "home.subscript",
    "home.superscript",
    "home.wrapText",
];

/// The commands that work on suggestions themselves (never suggested).
pub const REVIEW_COMMANDS: &[&str] =
    &["review.acceptSuggestion", "review.rejectSuggestion", "review.acceptAllSuggestions", "review.rejectAllSuggestions"];

/// One suggested change of a cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    /// The sheet, by name.
    pub sheet: String,
    /// The cell, in A1 form.
    pub cell: String,
    /// What the cell held before (nothing: it was empty).
    pub before: Option<Cell>,
    pub author: String,
    /// Milliseconds since the Unix epoch.
    pub time_ms: u64,
}

impl Suggestion {
    fn at(&self) -> Option<CellRef> {
        CellRef::parse(&self.cell)
    }
}

/// The workbook's suggestions, oldest first.
pub fn suggestions(wb: &Workbook) -> Vec<Suggestion> {
    stash::get_json(wb, KEY).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn store(wb: &mut Workbook, list: &[Suggestion]) -> Result<()> {
    if list.is_empty() {
        stash::remove(wb, KEY);
        return Ok(());
    }
    stash::set_json(wb, KEY, &serde_json::to_value(list).map_err(|e| EngineError::Other(e.to_string()))?)
}

/// What a cell held, as it would be typed: its formula, else its value.
pub fn input_text(cell: &Cell) -> String {
    match &cell.formula {
        Some(f) => format!("={}", f.text),
        None => cell.value.to_text().unwrap_or_default(),
    }
}

/// `cell` with its formula parsed again (a stored one comes back as text).
fn revived(mut cell: Cell) -> Cell {
    if let Some(f) = cell.formula.take() {
        let mut parsed = Formula::new(&f.text);
        parsed.array = f.array;
        parsed.dynamic = f.dynamic;
        cell.formula = Some(Arc::new(parsed));
    }
    cell
}

/// Whether a cell's content or formatting changed (not only a formula's result).
fn edited(before: Option<&Cell>, after: Option<&Cell>) -> bool {
    let typed = |c: Option<&Cell>| {
        c.map(|c| (c.formula.as_ref().map(|f| f.text.clone()), c.style, c.formula.is_none().then(|| c.value.clone()), c.py.clone()))
    };
    typed(before) != typed(after)
}

/// Records, in `after`, a suggestion by `author` at `time_ms` for each cell that differs from
/// `before`. A cell suggested before keeps what it held first; its author becomes the latest.
/// An edit that changed more than cells (sheets added, removed or renamed) can't be suggested.
pub(crate) fn record(before: &Workbook, after: &mut Workbook, author: &str, time_ms: u64) -> Result<()> {
    if before.sheets.len() != after.sheets.len() || before.sheets.iter().zip(&after.sheets).any(|(a, b)| a.name != b.name) {
        return Err(EngineError::Other("Only changes to cells can be suggested".into()));
    }
    let mut list = suggestions(after);
    let mut grew = false;
    for (b, a) in before.sheets.iter().zip(&after.sheets) {
        if Arc::ptr_eq(b, a) {
            continue;
        }
        for c in b.cells.diff(&a.cells) {
            // A formula's result moving with what it reads isn't an edit of that cell.
            if !edited(b.cell(c), a.cell(c)) {
                continue;
            }
            let cell = c.a1();
            match list.iter_mut().find(|s| s.sheet == a.name && s.cell == cell) {
                Some(s) => s.author = author.to_string(),
                None => list.push(Suggestion { sheet: a.name.clone(), cell, before: b.cell(c).cloned(), author: author.to_string(), time_ms }),
            }
            grew = true;
        }
    }
    if grew { store(after, &list) } else { Ok(()) }
}

/// Which suggestions a review command reaches: at `sheet`/`cell` when given, else the active
/// cell's (for one), or everyone's or `author`'s (for all).
fn chosen(s: &Session, p: &Json, all: bool) -> Result<Vec<Suggestion>> {
    let d = s.doc()?;
    let list = suggestions(&d.wb);
    let sheet = match str_param(p, "sheet") {
        Some(name) => name.to_string(),
        None => d.wb.active().map(|sh| sh.name.clone()).unwrap_or_default(),
    };
    if all {
        let author = str_param(p, "author");
        return Ok(list.into_iter().filter(|x| author.is_none_or(|a| x.author == a)).collect());
    }
    let cell = match str_param(p, "cell") {
        Some(c) => CellRef::parse(c).ok_or_else(|| EngineError::Other(format!("“{c}” isn't a cell")))?,
        None => d.selection.active,
    };
    Ok(list.into_iter().filter(|x| x.sheet == sheet && x.at() == Some(cell)).collect())
}

/// Accepts (`keep`) or rejects the suggestions `p` names: rejecting puts back what each cell held.
fn review(s: &mut Session, p: &Json, all: bool, keep: bool) -> Result<Json> {
    let chosen = chosen(s, p, all)?;
    if chosen.is_empty() {
        return Ok(json!({"count": 0}));
    }
    let count = chosen.len();
    edit(s, |cx| {
        if !keep {
            for x in &chosen {
                let (Some(i), Some(c)) = (cx.wb.sheet_index(&x.sheet), x.at()) else { continue };
                let sh = cx.sheet_mut(i)?;
                match &x.before {
                    Some(cell) => sh.set_cell(c, revived(cell.clone())),
                    None => {
                        sh.remove_cell(c);
                    }
                }
                cx.touch(i, c);
            }
        }
        let rest: Vec<Suggestion> = suggestions(&cx.wb).into_iter().filter(|x| !chosen.contains(x)).collect();
        store(&mut cx.wb, &rest)?;
        Ok(json!({"count": count}))
    })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "review.suggesting", "Suggesting", ["Review", "Suggesting"], None, "{on?: bool (default: toggle)} → {on}", has_doc, |s, p| {
            s.suggesting = bool_param(p, "on").unwrap_or(!s.suggesting);
            Ok(json!({"on": s.suggesting}))
        }),
        cmd!(query "review.suggestions", "Suggestions", ["Review", "Suggesting"], None, "{} → {suggestions: [{sheet, cell, author, timeMs, before}]}", has_doc, |s, _| {
            let list: Vec<Json> = suggestions(&s.doc()?.wb)
                .into_iter()
                .map(|x| json!({"sheet": x.sheet, "cell": x.cell, "author": x.author, "timeMs": x.time_ms, "before": x.before.as_ref().map(input_text)}))
                .collect();
            Ok(json!({"suggestions": list}))
        }),
        cmd!(
            "review.acceptSuggestion",
            "Accept Suggestion",
            ["Review", "Suggesting"],
            None,
            "{sheet?, cell?} (default: the active cell) → {count}",
            has_doc,
            |s, p| review(s, p, false, true)
        ),
        cmd!(
            "review.rejectSuggestion",
            "Reject Suggestion",
            ["Review", "Suggesting"],
            None,
            "{sheet?, cell?} (default: the active cell) → {count}",
            has_doc,
            |s, p| review(s, p, false, false)
        ),
        cmd!(
            "review.acceptAllSuggestions",
            "Accept All Suggestions",
            ["Review", "Suggesting"],
            None,
            "{author?} (default: everyone's) → {count}",
            has_doc,
            |s, p| review(s, p, true, true)
        ),
        cmd!(
            "review.rejectAllSuggestions",
            "Reject All Suggestions",
            ["Review", "Suggesting"],
            None,
            "{author?} (default: everyone's) → {count}",
            has_doc,
            |s, p| review(s, p, true, false)
        ),
    ]
}

#[cfg(test)]
#[path = "suggest_tests.rs"]
mod tests;
