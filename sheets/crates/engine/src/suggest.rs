//! Suggesting: edits made as suggestions, which anyone who may edit accepts or rejects, cell by
//! cell, range by range, person by person or all at once (as Craft Docs' Suggesting mode; a
//! suggester can't review, there or here).
//!
//! A suggested edit shows at once, as typed; what each cell held before is kept with who
//! suggested it and when. Accepting keeps the cell as it is and drops the record; rejecting puts
//! back what the cell held. The records are part of the workbook (a hidden item, see
//! [`crate::cmd::stash`]), so they travel in its .xlsx, in co-editing snapshots and in undo
//! steps, and every replica records the same ones: an edit replayed with `track` on its context
//! records them as it runs.
//!
//! Only edits of cells' contents and formatting can be suggested ([`SUGGEST_COMMANDS`]): rejecting
//! one of those puts the workbook back exactly (one Reject All test per command proves it). An
//! edit that also changes anything outside cells (notes, links, merges, whole rows' or columns'
//! formats), structural edits (rows, columns, sheets) and the rest are refused while suggesting.
//!
//! Records follow the workbook: a structural edit moves them with their cells (and their earlier
//! formulas with the references in them), a deleted cell's record goes, and a direct edit of a
//! suggested cell settles its suggestion (the cell is what that person made it).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gridcraft_core::CellRef;
use gridcraft_model::structure::Structural;
use gridcraft_model::{Cell, Formula, Sheet, Style, Workbook};
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
    /// Its format then, by value: another program saving the file renumbers the workbook's styles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<Style>,
    pub author: String,
    /// Milliseconds since the Unix epoch.
    pub time_ms: u64,
}

/// The most cells one suggested edit may change.
pub const MAX_SUGGESTED_CELLS: usize = 10_000;

impl Suggestion {
    fn at(&self) -> Option<CellRef> {
        CellRef::parse(&self.cell)
    }
}

/// The workbook's suggestions, oldest first. A record this version can't read (written by a newer
/// one) is left out here and kept in the workbook ([`store`]).
pub fn suggestions(wb: &Workbook) -> Vec<Suggestion> {
    raw(wb).into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect()
}

/// [`suggestions`] for painting, read again only when the records change: the front end asks
/// every frame. Keyed on the stored text's names (where each is and how it ends), which any change
/// of the records replaces.
pub fn cached(wb: &Workbook) -> Arc<Vec<Suggestion>> {
    thread_local! {
        static CACHE: std::cell::RefCell<Option<(u64, Arc<Vec<Suggestion>>)>> = const { std::cell::RefCell::new(None) };
    }
    if !any(wb) {
        return Arc::new(Vec::new());
    }
    let prefix = format!("_gc_{KEY}_");
    let mut key: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        key ^= v;
        key = key.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for n in wb.names.iter().filter(|n| n.name.to_ascii_lowercase().starts_with(&prefix)) {
        mix(n.formula.as_ptr() as u64);
        mix(n.formula.len() as u64);
        let tail = n.formula.len().saturating_sub(256);
        for b in n.formula.as_bytes().get(tail..).unwrap_or_default() {
            mix(u64::from(*b));
        }
    }
    CACHE.with(|c| {
        if let Some((k, list)) = &*c.borrow()
            && *k == key
        {
            return list.clone();
        }
        let list = Arc::new(suggestions(wb));
        *c.borrow_mut() = Some((key, list.clone()));
        list
    })
}

/// The stored records as they are.
fn raw(wb: &Workbook) -> Vec<Json> {
    match stash::get_json(wb, KEY) {
        Some(Json::Array(v)) => v,
        _ => Vec::new(),
    }
}

/// Whether the workbook holds suggestions, without reading them.
pub fn any(wb: &Workbook) -> bool {
    stash::holds_item(wb, &format!("_gc_{KEY}_0"))
}

fn store(wb: &mut Workbook, list: &[Suggestion]) -> Result<()> {
    let mut all: Vec<Json> = raw(wb).into_iter().filter(|v| serde_json::from_value::<Suggestion>(v.clone()).is_err()).collect();
    for x in list {
        all.push(serde_json::to_value(x).map_err(|e| EngineError::Other(e.to_string()))?);
    }
    if all.is_empty() {
        stash::remove(wb, KEY);
        return Ok(());
    }
    stash::set_json(wb, KEY, &Json::Array(all))
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

/// Whether two versions of a sheet differ only in their cells (contents, computed spills, the view).
fn same_but_cells(a: &Sheet, b: &Sheet) -> bool {
    let Sheet {
        name,
        uid: _,
        file_id,
        cells: _,
        cell_pictures,
        spill: _,
        spill_ranges: _,
        cols,
        rows,
        default_col_width,
        default_row_height,
        merges,
        freeze,
        tab_color,
        visibility,
        show_gridlines,
        show_headings,
        show_formulas,
        show_zeros,
        show_outline_symbols,
        summary_below,
        summary_right,
        right_to_left,
        zoom: _,
        cond_formats,
        validations,
        tables,
        autofilter,
        comments,
        hyperlinks,
        charts,
        images,
        shapes,
        sparklines,
        slicers,
        pivots,
        protection,
        edit_ranges,
        print,
        view_active: _,
        view_top_left: _,
        view: _,
        preserved,
    } = a;
    *name == b.name
        && *file_id == b.file_id
        && *cell_pictures == b.cell_pictures
        && *cols == b.cols
        && same_rows(rows, &b.rows)
        && *default_col_width == b.default_col_width
        && *default_row_height == b.default_row_height
        && *merges == b.merges
        && *freeze == b.freeze
        && *tab_color == b.tab_color
        && *visibility == b.visibility
        && *show_gridlines == b.show_gridlines
        && *show_headings == b.show_headings
        && *show_formulas == b.show_formulas
        && *show_zeros == b.show_zeros
        && *show_outline_symbols == b.show_outline_symbols
        && *summary_below == b.summary_below
        && *summary_right == b.summary_right
        && *right_to_left == b.right_to_left
        && *cond_formats == b.cond_formats
        && *validations == b.validations
        && *tables == b.tables
        && *autofilter == b.autofilter
        && *comments == b.comments
        && *hyperlinks == b.hyperlinks
        && *charts == b.charts
        && *images == b.images
        && *shapes == b.shapes
        && *sparklines == b.sparklines
        && *slicers == b.slicers
        && *pivots == b.pivots
        && *protection == b.protection
        && *edit_ranges == b.edit_ranges
        && *print == b.print
        && *preserved == b.preserved
}

/// Whether two sheets' row settings are the same but for automatic heights, which follow what the
/// cells hold.
fn same_rows(a: &std::collections::BTreeMap<u32, gridcraft_model::LineInfo>, b: &std::collections::BTreeMap<u32, gridcraft_model::LineInfo>) -> bool {
    let set = |l: &gridcraft_model::LineInfo| {
        let mut l = *l;
        if !l.custom {
            l.size = None;
        }
        (l != gridcraft_model::LineInfo::default()).then_some(l)
    };
    let pick = |m: &std::collections::BTreeMap<u32, gridcraft_model::LineInfo>| m.iter().filter_map(|(k, l)| Some((*k, set(l)?))).collect::<Vec<_>>();
    pick(a) == pick(b)
}

/// Whether two versions of a workbook differ only in their sheets and in styles added.
fn same_but_sheets(a: &Workbook, b: &Workbook) -> bool {
    let rest = |w: &Workbook| Workbook { sheets: Vec::new(), styles: Default::default(), ..w.clone() };
    let styles_kept = (0..a.styles.len()).all(|i| {
        let id = gridcraft_model::StyleId(i as u32);
        a.styles.get(id) == b.styles.get(id)
    });
    styles_kept && rest(a) == rest(b)
}

/// Records, in `after`, a suggestion by `author` at `time_ms` for each cell that differs from
/// `before`. A cell suggested before keeps what it held first; its author becomes the latest.
/// An edit that changed anything but cells (sheets, notes, links, merges, whole rows' or
/// columns' formats) can't be suggested, nor one of more than [`MAX_SUGGESTED_CELLS`] cells.
pub(crate) fn record(before: &Workbook, after: &mut Workbook, author: &str, time_ms: u64) -> Result<()> {
    let only_cells = || EngineError::Other("Only changes to cells' contents and formatting can be suggested".into());
    if before.sheets.len() != after.sheets.len() || !same_but_sheets(before, after) {
        return Err(only_cells());
    }
    let mut changed = Vec::new();
    for (b, a) in before.sheets.iter().zip(&after.sheets) {
        if Arc::ptr_eq(b, a) {
            continue;
        }
        if !same_but_cells(b, a) {
            return Err(only_cells());
        }
        // A formula's result moving with what it reads isn't an edit of that cell.
        changed.extend(b.cells.diff(&a.cells).into_iter().filter(|c| edited(b.cell(*c), a.cell(*c))).map(|c| (b, c)));
        if changed.len() > MAX_SUGGESTED_CELLS {
            return Err(EngineError::Other(format!("Suggest changes to at most {MAX_SUGGESTED_CELLS} cells at a time")));
        }
    }
    if changed.is_empty() {
        return Ok(());
    }
    let mut list = suggestions(after);
    let mut at: HashMap<(String, String), usize> = list.iter().enumerate().map(|(i, x)| ((x.sheet.clone(), x.cell.clone()), i)).collect();
    for (b, c) in changed {
        let key = (b.name.clone(), c.a1());
        match at.get(&key) {
            Some(&i) => list[i].author = author.to_string(),
            None => {
                let was = b.cell(c).cloned();
                let style = was.as_ref().map(|cell| before.styles.get(cell.style).clone());
                at.insert(key.clone(), list.len());
                list.push(Suggestion { sheet: key.0, cell: key.1, before: was, style, author: author.to_string(), time_ms });
            }
        }
    }
    store(after, &list)
}

/// `cell`'s formula after structural edit `edit` on sheet `target`, for a cell on sheet `host`.
fn adjusted(cell: &mut Cell, host: &str, e: &Structural) {
    let Some(f) = cell.formula.as_ref() else { return };
    let Some(expr) = f.expr() else { return };
    let expr = match e {
        Structural::Edit { sheet, edit } => gridcraft_formula::adjust::adjust(expr, host, sheet, edit),
        Structural::Renamed { from, to } => gridcraft_formula::adjust::rename_sheet(expr, from, to),
        Structural::Removed { .. } => return,
    };
    let mut nf = Formula::from_expr(expr);
    nf.array = f.array;
    nf.dynamic = f.dynamic;
    cell.formula = Some(Arc::new(nf));
}

/// The records after `after`'s edits since `before`, made directly (not as suggestions): moved
/// with their cells through structural edits, gone with deleted cells and sheets, and settled
/// where the edit changed a suggested cell. Whether anything changed.
pub(crate) fn follow(before: &Workbook, after: &mut Workbook) -> Result<bool> {
    let mut list = suggestions(after);
    let n = list.len();
    match after.structure.since(&before.structure) {
        Some(edits) if !edits.is_empty() => {
            list.retain_mut(|x| {
                for e in &edits {
                    match e {
                        Structural::Edit { sheet, edit } => {
                            if let Some(cell) = &mut x.before {
                                adjusted(cell, &x.sheet, e);
                            }
                            if *sheet != x.sheet {
                                continue;
                            }
                            let Some(c) = x.at() else { return false };
                            if gridcraft_formula::adjust::cell_deleted(c, edit) {
                                return false;
                            }
                            if let gridcraft_formula::adjust::Edit::MoveToSheet { from, to_sheet, .. } = edit
                                && from.contains(c)
                            {
                                x.sheet = to_sheet.clone();
                            }
                            x.cell = gridcraft_formula::adjust::map_cell(c, edit).a1();
                        }
                        Structural::Renamed { from, to } => {
                            if let Some(cell) = &mut x.before {
                                adjusted(cell, &x.sheet, e);
                            }
                            if x.sheet == *from {
                                x.sheet = to.clone();
                            }
                        }
                        Structural::Removed { name, .. } if *name == x.sheet => return false,
                        Structural::Removed { .. } => {}
                    }
                }
                true
            });
            store(after, &list)?;
            return Ok(true);
        }
        _ => {}
    }
    // Cells edited directly: their suggestions are settled.
    let mut edited_cells: HashSet<(String, String)> = HashSet::new();
    for (b, a) in before.sheets.iter().zip(&after.sheets) {
        if Arc::ptr_eq(b, a) || b.name != a.name {
            continue;
        }
        for c in b.cells.diff(&a.cells) {
            if edited(b.cell(c), a.cell(c)) {
                edited_cells.insert((a.name.clone(), c.a1()));
            }
        }
    }
    list.retain(|x| !edited_cells.contains(&(x.sheet.clone(), x.cell.clone())));
    if list.len() == n {
        return Ok(false);
    }
    store(after, &list)?;
    Ok(true)
}

/// Which suggestions a review command reaches: at `sheet`/`cell` when given, else the selected
/// cells' (for one), or everyone's or `author`'s (for all).
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
    let cells: Vec<gridcraft_core::RangeRef> = match str_param(p, "cell") {
        Some(c) => {
            let c = CellRef::parse(c).ok_or_else(|| EngineError::Other(format!("“{c}” isn't a cell")))?;
            vec![gridcraft_core::RangeRef::new(c, c)]
        }
        None => d.selection.ranges.clone(),
    };
    let mut out: Vec<Suggestion> =
        list.iter().filter(|x| x.sheet == sheet && x.at().is_some_and(|c| cells.iter().any(|r| r.contains(c)))).cloned().collect();
    // An array formula's cells are put back together, or not at all.
    let arrays: Vec<gridcraft_core::RangeRef> = out
        .iter()
        .filter_map(|x| {
            let now =
                d.wb.sheet_index(&x.sheet)
                    .and_then(|i| d.wb.sheet(i))
                    .and_then(|sh| x.at().and_then(|c| sh.cell(c)).and_then(|c| c.formula.as_ref()?.array));
            x.before.as_ref().and_then(|c| c.formula.as_ref()?.array).into_iter().chain(now).next()
        })
        .collect();
    if !arrays.is_empty() {
        for x in &list {
            if x.sheet == sheet && x.at().is_some_and(|c| arrays.iter().any(|r| r.contains(c))) && !out.contains(x) {
                out.push(x.clone());
            }
        }
    }
    Ok(out)
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
                match &x.before {
                    Some(cell) => {
                        let mut cell = revived(cell.clone());
                        if let Some(style) = &x.style {
                            cell.style = cx.wb.styles.intern(style.clone());
                        }
                        cx.sheet_mut(i)?.set_cell(c, cell);
                    }
                    None => {
                        cx.sheet_mut(i)?.remove_cell(c);
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
            "{sheet?, cell?} (default: the selected cells) → {count}",
            has_doc,
            |s, p| review(s, p, false, true)
        ),
        cmd!(
            "review.rejectSuggestion",
            "Reject Suggestion",
            ["Review", "Suggesting"],
            None,
            "{sheet?, cell?} (default: the selected cells) → {count}",
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
