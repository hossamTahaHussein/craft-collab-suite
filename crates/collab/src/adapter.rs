//! WordCraft as a co-editable document for the suite's co-editing core: a selection is a pair
//! of story offsets ([`Anchor`]), an edit's context is the engine's `RunCtx`, and what a
//! revision changed is a span of linear text per story (`mapping`).

use serde_json::Value;
use wordcraft_doc::offsets::{offset_of, pos_at};
use wordcraft_doc::{Document, Pos, StoryRef};
use wordcraft_engine::{RunCtx, Selection, Session};

use craft_collab_core::doc::{CollabDoc, Local};
use craft_collab_core::proto::OpKind;

use crate::mapping::{Span, changes, map, story_diff};
use crate::proto::{Anchor, HistSpan, Op, Spans};

/// The longest merged run of typing, in bytes.
const MAX_MERGED_TEXT: usize = 4096;

/// WordCraft in the co-editing core.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocsDoc;

impl CollabDoc for DocsDoc {
    type Doc = Document;
    type Session = Session;
    type Sel = Anchor;
    type Ctx = RunCtx;
    type Change = Vec<(StoryRef, Span)>;
    type Step = Span;
    type Wire = Spans;
    type Hint = (StoryRef, usize);

    fn headless(doc: Document) -> Session {
        let mut s = Session::new(doc);
        s.set_recording(false);
        s
    }

    fn doc(s: &Session) -> &Document {
        &s.doc
    }

    fn set_doc(s: &mut Session, doc: Document) {
        s.doc = doc;
    }

    fn attach(visible: &mut Session, doc: &Document) {
        visible.doc = doc.clone();
        visible.sel = Selection::caret(Pos::body(0, 0));
        visible.clamp_selection();
        visible.set_collab(true);
        visible.touch();
    }

    fn take_local(visible: &mut Session) -> Vec<Local<DocsDoc>> {
        visible
            .take_outbox()
            .into_iter()
            .map(|r| Local {
                cmd: r.id,
                params: r.params,
                sel: r.anchor.map(|(story, anchor, head)| Anchor { story, anchor, head }).unwrap_or_default(),
                ctx: r.ctx,
            })
            .collect()
    }

    fn selection(s: &Session) -> Option<Anchor> {
        anchor_of(&s.doc, &s.sel)
    }

    fn select(s: &mut Session, sel: &Anchor) {
        s.sel = selection_at(&s.doc, sel);
    }

    fn clamp(s: &mut Session) {
        s.clamp_selection();
    }

    fn settle(visible: &mut Session) {
        visible.clamp_selection();
        visible.touch();
    }

    fn begin_replay(visible: &mut Session) -> bool {
        visible.set_recording(false);
        // Edits made before Viewing was turned on are replayed, not refused as new ones.
        std::mem::replace(&mut visible.viewing, false)
    }

    fn end_replay(visible: &mut Session, viewing: bool) {
        visible.set_recording(true);
        visible.viewing = viewing;
    }

    fn run_at(s: &mut Session, op: &Op, sel: &Anchor) -> bool {
        run_at(s, op, sel).is_ok()
    }

    fn restore_into(s: &mut Session, op: &Op) {
        restore_into(s, op);
    }

    fn changes(before: &Document, after: &Document, hint: Option<(StoryRef, usize)>) -> Vec<(StoryRef, Span)> {
        changes(before, after, hint)
    }

    fn hint_at(sel: &Anchor) -> Option<(StoryRef, usize)> {
        Some((sel.story, sel.anchor.min(sel.head)))
    }

    fn hint_in(change: &Vec<(StoryRef, Span)>, sel: &Anchor) -> Option<(StoryRef, usize)> {
        change.iter().find(|(story, _)| *story == sel.story).map(|(story, s)| (*story, s.start))
    }

    fn map(sel: &Anchor, change: &Vec<(StoryRef, Span)>) -> Anchor {
        let mut a = sel.clone();
        for (story, s) in change {
            if *story == a.story {
                a.anchor = map(s, a.anchor, false);
                a.head = map(s, a.head, false);
            }
        }
        a
    }

    fn step(before: &Document, after: &Document, sel: &Anchor, hint: Option<(StoryRef, usize)>) -> Option<Span> {
        story_diff(before, after, sel.story, hint)
    }

    fn move_sel(sel: &Anchor, step: Option<&Span>) -> Anchor {
        move_anchor(sel, step)
    }

    fn move_hint(hint: Option<(StoryRef, usize)>, sel: &Anchor, own: &Span) -> Option<(StoryRef, usize)> {
        match hint {
            Some((story, h)) if story == sel.story => Some((story, map(own, h, false))),
            other => other,
        }
    }

    fn after_insertion(sel: &Anchor, sp: &Span) -> Option<Anchor> {
        (sp.old_end == sp.start && sel.anchor == sp.start && sel.head == sp.start).then_some(Anchor {
            story: sel.story,
            anchor: sp.new_end,
            head: sp.new_end,
        })
    }

    fn to_wire(change: &Vec<(StoryRef, Span)>) -> Spans {
        Spans { spans: change.iter().map(|(story, s)| HistSpan { story: *story, start: s.start, old_end: s.old_end, new_end: s.new_end }).collect() }
    }

    fn from_wire(wire: Spans) -> Vec<(StoryRef, Span)> {
        wire.spans.into_iter().map(|s| (s.story, Span { start: s.start, old_end: s.old_end, new_end: s.new_end })).collect()
    }

    fn hash(doc: &Document) -> String {
        crate::hash::doc_hash(doc)
    }

    /// The document in WordCraft's native JSON.
    fn encode(doc: &Document) -> Result<String, String> {
        let bytes = wordcraft_engine::io::save_bytes("doc.json", doc)?;
        String::from_utf8(bytes).map_err(|e| e.to_string())
    }

    fn is_typing(op: &Op) -> bool {
        op.cmd == "text.insert"
    }

    /// Plain typing: consecutive `text.insert`s of letters and digits, each right after the
    /// last, untracked, by the same person.
    fn merge(a: &mut Op, b: &Op) -> bool {
        let (Some(ta), Some(tb)) = (plain_typing(a), plain_typing(b)) else { return false };
        let start = a.sel.anchor.min(a.sel.head);
        let joins = b.sel.story == a.sel.story
            && b.sel.anchor == b.sel.head
            && b.sel.anchor == start + ta.len()
            && !a.ctx.track
            && !b.ctx.track
            && a.ctx.author == b.ctx.author
            && ta.len() + tb.len() <= MAX_MERGED_TEXT;
        if !joins {
            return false;
        }
        a.params = serde_json::json!({ "text": format!("{ta}{tb}") });
        true
    }
}

/// The text of a `text.insert` of letters and digits only (typing that can't trigger
/// AutoCorrect or AutoFormat, so one insert of all of it gives the same document).
fn plain_typing(op: &Op) -> Option<String> {
    if op.cmd != "text.insert" || op.kind != OpKind::Cmd {
        return None;
    }
    let obj = op.params.as_object()?;
    if obj.len() != 1 {
        return None;
    }
    let text = obj.get("text").and_then(Value::as_str)?;
    (!text.is_empty() && text.chars().all(char::is_alphanumeric)).then(|| text.to_string())
}

/// Put back the document a restore edit carries (`params.doc`, native JSON); one that can't be
/// read changes nothing, on every replica.
pub(crate) fn restore_into(s: &mut Session, op: &Op) {
    let Some(json) = op.params.get("doc").and_then(Value::as_str) else { return };
    if let Ok(doc) = wordcraft_engine::io::open_bytes("version.json", json.as_bytes()) {
        s.doc = doc;
        s.clamp_selection();
    }
}

/// Run `op` in `s` with its context, from `sel`.
pub(crate) fn run_at(s: &mut Session, op: &Op, sel: &Anchor) -> Result<Value, wordcraft_engine::CmdError> {
    s.sel = selection_at(&s.doc, sel);
    s.clamp_selection();
    s.run_with(&op.ctx, &op.cmd, &op.params)
}

fn selection_at(doc: &Document, a: &Anchor) -> Selection {
    Selection { anchor: pos_at(doc, a.story, a.anchor), focus: pos_at(doc, a.story, a.head) }
}

fn anchor_of(doc: &Document, sel: &Selection) -> Option<Anchor> {
    if sel.anchor.story != sel.focus.story {
        return None;
    }
    Some(Anchor { story: sel.anchor.story, anchor: offset_of(doc, &sel.anchor)?, head: offset_of(doc, &sel.focus)? })
}

pub(crate) fn move_anchor(a: &Anchor, s: Option<&Span>) -> Anchor {
    match s {
        Some(s) => Anchor { story: a.story, anchor: map(s, a.anchor, false), head: map(s, a.head, false) },
        None => a.clone(),
    }
}
