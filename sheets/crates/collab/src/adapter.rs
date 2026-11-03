//! A Craft Sheets workbook as the suite's co-editing core sees it: a selection is a sheet (by
//! name) and the selection on it, an edit's context is the engine's `collab::RunCtx`, and what a
//! revision changed is the structural edits it made (`gridcraft_model::structure`), which is what
//! moves cell positions; cell edits move nothing.

use std::sync::{Arc, LazyLock};

use craft_collab_core::doc::{CollabDoc, Local};
use craft_collab_core::proto::Op;
use gridcraft_core::RangeRef;
use gridcraft_engine::Session;
use gridcraft_engine::collab::{RunCtx, SheetPlace};
use gridcraft_model::Workbook;
use gridcraft_model::structure::Structural;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mapping::{map_range_through, map_through};
use crate::wire::Edits;

/// Command parameters that name cells (`"B2"`, `"A1:C4"`, `"3:5"`, `"B:D"`), on the sheet the
/// command runs on: they move with the cells, as the selection does, before another replica
/// runs the edit. Content (`input`, `text`, `value`, …) never does.
pub const ADDRESS_KEYS: &[&str] = &[
    "at",
    "range",
    "cell",
    "cols",
    "rows",
    "column",
    "to",
    "destination",
    "source",
    "target",
    "changing",
    "set",
    "rowInput",
    "columnInput",
    "resultCells",
    "reference",
    "printArea",
    "output",
    "objective",
    "location",
    "lhs",
    "fromCell",
    "from",
    "criteria",
    "copyTo",
    "active",
    "apply",
];

/// Where an edit runs: the sheet and selection, and the cells its parameters name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SheetSel {
    #[serde(flatten)]
    pub place: SheetPlace,
    /// `(parameter, cells)` for each [`ADDRESS_KEYS`] parameter the command was given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<(String, RangeRef)>,
}

impl SheetSel {
    /// The place an edit with `params` ran at, with the cells its parameters name.
    pub fn of(place: SheetPlace, params: &Value) -> SheetSel {
        let refs = ADDRESS_KEYS
            .iter()
            .filter_map(|k| params.get(*k).and_then(Value::as_str).and_then(RangeRef::parse).map(|r| (k.to_string(), r)))
            .collect();
        SheetSel { place, refs }
    }

    /// `params` with each named parameter set to where its cells are now.
    pub fn params(&self, params: &Value) -> Value {
        let mut out = params.clone();
        if let Some(obj) = out.as_object_mut() {
            for (k, r) in &self.refs {
                obj.insert(k.clone(), Value::String(r.a1()));
            }
        }
        out
    }
}

/// What a session with no workbook shows the core (it never has one while co-editing).
static EMPTY: LazyLock<Arc<Workbook>> = LazyLock::new(|| Arc::new(Workbook::new()));

/// Craft Sheets in the co-editing core.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetsDoc;

pub type SheetOp = Op<SheetsDoc>;

impl CollabDoc for SheetsDoc {
    type Doc = Arc<Workbook>;
    type Session = Session;
    type Sel = SheetSel;
    type Ctx = RunCtx;
    type Change = Vec<Structural>;
    type Step = Vec<Structural>;
    type Wire = Edits;
    type Hint = ();

    fn headless(doc: Arc<Workbook>) -> Session {
        let mut s = Session::new();
        s.set_collab_workbook(doc);
        s.set_recording(false);
        s
    }

    fn doc(s: &Session) -> &Arc<Workbook> {
        s.active().map_or(&EMPTY, |d| &d.wb)
    }

    fn set_doc(s: &mut Session, doc: Arc<Workbook>) {
        s.set_collab_workbook(doc);
    }

    fn attach(visible: &mut Session, doc: &Arc<Workbook>) {
        visible.set_collab_workbook(doc.clone());
        let first = doc.sheets.first().map(|s| s.name.clone()).unwrap_or_default();
        visible.set_collab_place(&SheetPlace { sheet: first, selection: Default::default() });
        visible.set_collab(true);
    }

    fn take_local(visible: &mut Session) -> Vec<Local<SheetsDoc>> {
        visible
            .take_outbox()
            .into_iter()
            .map(|r| {
                let sel = SheetSel::of(r.place.unwrap_or_default(), &r.params);
                Local { cmd: r.id, params: r.params, sel, ctx: r.ctx }
            })
            .collect()
    }

    fn selection(s: &Session) -> Option<SheetSel> {
        s.collab_place().map(|place| SheetSel { place, refs: Vec::new() })
    }

    fn select(s: &mut Session, sel: &SheetSel) {
        s.set_collab_place(&sel.place);
    }

    /// Cell references are always inside the sheet; a sheet that is gone leaves the active one.
    fn clamp(_s: &mut Session) {}

    fn settle(_visible: &mut Session) {}

    fn begin_replay(visible: &mut Session) -> bool {
        let was = visible.recording();
        visible.set_recording(false);
        was
    }

    fn end_replay(visible: &mut Session, saved: bool) {
        visible.set_recording(saved);
    }

    fn run_at(s: &mut Session, op: &SheetOp, sel: &SheetSel) -> bool {
        s.set_collab_place(&sel.place);
        s.execute_with(&op.ctx, &op.cmd, sel.params(&op.params)).is_ok()
    }

    /// The kept version a restore carries (`params.doc`, native JSON); one that can't be read
    /// changes nothing, on every replica.
    fn restore_into(s: &mut Session, op: &SheetOp) {
        let Some(json) = op.params.get("doc").and_then(Value::as_str) else { return };
        if let Some(wb) = decode(json) {
            s.set_collab_workbook(Arc::new(wb));
        }
    }

    /// The structural edits made since `before` (none for cell edits, or when `after` doesn't
    /// continue `before`'s log: a restored version).
    fn changes(before: &Arc<Workbook>, after: &Arc<Workbook>, _hint: Option<()>) -> Vec<Structural> {
        after.structure.since(&before.structure).unwrap_or_default()
    }

    fn hint_at(_sel: &SheetSel) -> Option<()> {
        None
    }

    fn hint_in(_change: &Vec<Structural>, _sel: &SheetSel) -> Option<()> {
        None
    }

    fn map(sel: &SheetSel, change: &Vec<Structural>) -> SheetSel {
        move_through(sel, change)
    }

    fn step(before: &Arc<Workbook>, after: &Arc<Workbook>, _sel: &SheetSel, _hint: Option<()>) -> Option<Vec<Structural>> {
        Some(Self::changes(before, after, None)).filter(|c| !c.is_empty())
    }

    fn move_sel(sel: &SheetSel, step: Option<&Vec<Structural>>) -> SheetSel {
        step.map_or_else(|| sel.clone(), |s| move_through(sel, s))
    }

    fn move_hint(_hint: Option<()>, _sel: &SheetSel, _own: &Vec<Structural>) -> Option<()> {
        None
    }

    fn after_insertion(_sel: &SheetSel, _step: &Vec<Structural>) -> Option<SheetSel> {
        None
    }

    fn to_wire(change: &Vec<Structural>) -> Edits {
        Edits::from(change.as_slice())
    }

    fn from_wire(wire: Edits) -> Vec<Structural> {
        wire.into()
    }

    /// The workbook's content: which sheet a window shows is each person's own view.
    fn hash(doc: &Arc<Workbook>) -> String {
        let content = (**doc).clone();
        content.set_active_sheet(0);
        let bytes = encode_bytes(&content).unwrap_or_default();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{h:016x}")
    }

    fn encode(doc: &Arc<Workbook>) -> Result<String, String> {
        String::from_utf8(encode_bytes(doc)?).map_err(|e| e.to_string())
    }

    /// Each entered cell is its own undo step, as in Excel.
    fn is_typing(_op: &SheetOp) -> bool {
        false
    }

    /// Typing in a cell stays in this editor until it is entered: there is nothing to merge.
    fn merge(_first: &mut SheetOp, _next: &SheetOp) -> bool {
        false
    }
}

/// `sel` after structural edits: its place, and the cells its parameters name (on its sheet).
fn move_through(sel: &SheetSel, edits: &[Structural]) -> SheetSel {
    let refs = sel.refs.iter().map(|(k, r)| (k.clone(), map_range_through(&sel.place.sheet, *r, edits))).collect();
    SheetSel { place: map_through(&sel.place, edits), refs }
}

fn encode_bytes(doc: &Workbook) -> Result<Vec<u8>, String> {
    gridcraft_engine::io::save_bytes(doc, "shared.json").map_err(|e| e.to_string())
}

/// A workbook from its native JSON (a snapshot, a kept version).
pub fn decode(json: &str) -> Option<Workbook> {
    gridcraft_engine::io::open_bytes("shared.json", json.as_bytes()).ok().map(|(wb, _)| wb)
}
