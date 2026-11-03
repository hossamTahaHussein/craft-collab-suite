//! A Craft Slides presentation as the suite's co-editing core sees it: a selection is a slide (by
//! id), the selection on it, its text caret as offsets in the text's linear form, and the slides
//! its parameters name by index (as ids); an edit's context is the engine's `collab::RunCtx`; what
//! a revision changed is [`SlideChange`].

use std::sync::{Arc, LazyLock};

use craft_collab_core::doc::{CollabDoc, Local};
use craft_collab_core::proto::Op;
use deckcraft_engine::Session;
use deckcraft_engine::collab::{RunCtx, SlidePlace};
use deckcraft_model::{Presentation, SlideId, TextBody, walk};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::change::{SlideChange, TextKey, changes, map_offset, offset_of, pos_at};

static EMPTY: LazyLock<Arc<Presentation>> = LazyLock::new(|| Arc::new(Presentation::default()));

/// Command parameters that name a slide by its index: they follow the slide, as the selection
/// does, before another replica runs the edit.
pub const SLIDE_INDEX_KEYS: &[&str] = &["slide", "index", "from", "to", "at"];

/// Where an edit runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SlideSel {
    #[serde(flatten)]
    pub place: SlidePlace,
    /// The text caret: `(text, anchor, caret)` as offsets in the text's linear form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<(TextKey, usize, usize)>,
    /// `(parameter, slide)` for each [`SLIDE_INDEX_KEYS`] parameter that named a slide.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slides: Vec<(String, SlideId)>,
}

/// The text body `key` names in `doc`.
fn body(doc: &Presentation, key: TextKey) -> Option<&TextBody> {
    match key {
        TextKey::Notes(slide) => doc.slides.iter().find(|s| s.id == slide).map(|s| &s.notes),
        TextKey::Shape(shape) => {
            let mut found = None;
            for s in &doc.slides {
                walk(&s.shapes, &mut |sh, _| {
                    if sh.id == shape {
                        found = sh.text.as_ref();
                    }
                });
                if found.is_some() {
                    break;
                }
            }
            found
        }
    }
}

impl SlideSel {
    /// The selection of `place` in `doc`, and the slides `params` name by index.
    pub fn of(place: SlidePlace, doc: &Presentation, params: &Value) -> SlideSel {
        let text = place.selection.text.as_ref().filter(|t| t.cell.is_none()).and_then(|t| {
            let key = if t.notes { TextKey::Notes(place.slide) } else { TextKey::Shape(t.shape) };
            let b = body(doc, key)?;
            Some((key, offset_of(b, t.anchor), offset_of(b, t.caret)))
        });
        let slides = SLIDE_INDEX_KEYS
            .iter()
            .filter_map(|k| {
                let i = params.get(*k)?.as_u64()? as usize;
                Some((k.to_string(), doc.slides.get(i)?.id))
            })
            .collect();
        SlideSel { place, text, slides }
    }

    /// `params` with each slide parameter at its slide's index now (one whose slide is gone keeps
    /// its number).
    pub fn params(&self, doc: &Presentation, params: &Value) -> Value {
        let mut out = params.clone();
        if let Some(obj) = out.as_object_mut() {
            for (k, id) in &self.slides {
                if let Some(i) = doc.slides.iter().position(|s| s.id == *id) {
                    obj.insert(k.clone(), Value::from(i));
                }
            }
        }
        out
    }
}

/// Craft Slides in the co-editing core.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlidesDoc;

pub type SlideOp = Op<SlidesDoc>;

fn select(s: &mut Session, sel: &SlideSel) {
    s.set_collab_place(&sel.place);
    let Some((key, a, c)) = sel.text else { return };
    let Some(d) = s.active() else { return };
    let Some(b) = body(&d.doc, key) else { return };
    let (anchor, caret) = (pos_at(b, a), pos_at(b, c));
    if let Some(d) = s.active_mut()
        && let Some(t) = d.selection.text.as_mut()
    {
        t.anchor = anchor;
        t.caret = caret;
        t.line_end = None;
    }
}

impl CollabDoc for SlidesDoc {
    type Doc = Arc<Presentation>;
    type Session = Session;
    type Sel = SlideSel;
    type Ctx = RunCtx;
    type Change = SlideChange;
    type Step = SlideChange;
    type Wire = SlideChange;
    type Hint = ();

    fn headless(doc: Arc<Presentation>) -> Session {
        let mut s = Session::new();
        s.set_collab_presentation(doc);
        s.set_recording(false);
        s
    }

    fn doc(s: &Session) -> &Arc<Presentation> {
        s.active().map_or(&EMPTY, |d| &d.doc)
    }

    fn set_doc(s: &mut Session, doc: Arc<Presentation>) {
        s.set_collab_presentation(doc);
    }

    fn attach(visible: &mut Session, doc: &Arc<Presentation>) {
        visible.set_collab_presentation(doc.clone());
        let first = doc.slides.first().map_or(SlideId(0), |s| s.id);
        visible.set_collab_place(&SlidePlace { slide: first, selection: Default::default() });
        visible.set_collab(true);
    }

    fn take_local(visible: &mut Session) -> Vec<Local<SlidesDoc>> {
        let now = Self::doc(visible).clone();
        visible
            .take_outbox()
            .into_iter()
            .map(|r| {
                let before = r.before.unwrap_or_else(|| now.clone());
                let sel = SlideSel::of(r.place.unwrap_or_default(), &before, &r.params);
                Local { cmd: r.id, params: r.params, sel, ctx: r.ctx }
            })
            .collect()
    }

    fn selection(s: &Session) -> Option<SlideSel> {
        let place = s.collab_place()?;
        Some(SlideSel::of(place, Self::doc(s), &Value::Null))
    }

    fn select(s: &mut Session, sel: &SlideSel) {
        select(s, sel);
    }

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

    fn run_at(s: &mut Session, op: &SlideOp, sel: &SlideSel) -> bool {
        select(s, sel);
        let params = sel.params(Self::doc(s), &op.params);
        s.execute_with(&op.ctx, &op.cmd, &params).is_ok()
    }

    /// The kept version a restore carries (`params.doc`, the native file in base64); one that
    /// can't be read changes nothing, on every replica.
    fn restore_into(s: &mut Session, op: &SlideOp) {
        let Some(text) = op.params.get("doc").and_then(Value::as_str) else { return };
        if let Some(doc) = decode(text) {
            s.set_collab_presentation(Arc::new(doc));
        }
    }

    fn changes(before: &Arc<Presentation>, after: &Arc<Presentation>, _hint: Option<()>) -> SlideChange {
        if Arc::ptr_eq(before, after) { SlideChange::default() } else { changes(before, after) }
    }

    fn hint_at(_sel: &SlideSel) -> Option<()> {
        None
    }

    fn hint_in(_change: &SlideChange, _sel: &SlideSel) -> Option<()> {
        None
    }

    fn map(sel: &SlideSel, ch: &SlideChange) -> SlideSel {
        let mut out = sel.clone();
        if let Some((_, next)) = ch.removed_slides.iter().find(|(id, _)| *id == sel.place.slide) {
            out.place.slide = *next;
            out.place.selection = Default::default();
            out.text = None;
        }
        out.place.selection.shapes.retain(|id| !ch.removed_shapes.contains(id));
        for (_, id) in &mut out.slides {
            if let Some((_, next)) = ch.removed_slides.iter().find(|(gone, _)| gone == id) {
                *id = *next;
            }
        }
        if let Some((key, a, c)) = out.text {
            let gone = match key {
                TextKey::Shape(id) => ch.removed_shapes.contains(&id),
                TextKey::Notes(slide) => ch.removed_slides.iter().any(|(s, _)| *s == slide),
            };
            if gone {
                out.text = None;
                out.place.selection.text = None;
            } else {
                let (mut a, mut c) = (a, c);
                for (k, s) in &ch.text {
                    if *k == key {
                        a = map_offset(a, s);
                        c = map_offset(c, s);
                    }
                }
                out.text = Some((key, a, c));
            }
        }
        out
    }

    fn step(before: &Arc<Presentation>, after: &Arc<Presentation>, _sel: &SlideSel, _hint: Option<()>) -> Option<SlideChange> {
        Some(Self::changes(before, after, None)).filter(|c| !c.is_empty())
    }

    fn move_sel(sel: &SlideSel, step: Option<&SlideChange>) -> SlideSel {
        step.map_or_else(|| sel.clone(), |s| Self::map(sel, s))
    }

    fn move_hint(_hint: Option<()>, _sel: &SlideSel, _own: &SlideChange) -> Option<()> {
        None
    }

    /// Redoing a run of typing: each piece goes after the one redone before it.
    fn after_insertion(sel: &SlideSel, step: &SlideChange) -> Option<SlideSel> {
        let (key, a, c) = sel.text?;
        let (_, s) = step.text.iter().find(|(k, s)| *k == key && s.old_end == s.start && a == s.start && c == s.start)?;
        Some(SlideSel { text: Some((key, s.new_end, s.new_end)), ..sel.clone() })
    }

    fn to_wire(change: &SlideChange) -> SlideChange {
        change.clone()
    }

    fn from_wire(wire: SlideChange) -> SlideChange {
        wire
    }

    fn hash(doc: &Arc<Presentation>) -> String {
        let bytes = serde_json::to_vec(&**doc).unwrap_or_default();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{h:016x}")
    }

    /// The native file, in base64.
    fn encode(doc: &Arc<Presentation>) -> Result<String, String> {
        let bytes = deckcraft_engine::cmd::file::save_bytes(doc, "deckcraft").map_err(|e| e.to_string())?;
        Ok(deckcraft_engine::cmd::base64_encode(&bytes))
    }

    fn is_typing(op: &SlideOp) -> bool {
        op.cmd == "text.insert"
    }

    /// Plain typing: consecutive `text.insert`s of letters and digits in one text, each right
    /// after the last, untracked, by the same person.
    fn merge(a: &mut SlideOp, b: &SlideOp) -> bool {
        let plain = |op: &SlideOp| -> Option<String> {
            let obj = op.params.as_object().filter(|o| o.len() == 1)?;
            let t = obj.get("text")?.as_str()?;
            (op.cmd == "text.insert" && !t.is_empty() && t.chars().all(char::is_alphanumeric)).then(|| t.to_string())
        };
        let (Some(ta), Some(tb)) = (plain(a), plain(b)) else { return false };
        let (Some((ka, aa, ac)), Some((kb, ba, bc))) = (a.sel.text, b.sel.text) else { return false };
        let joins = ka == kb
            && ba == bc
            && ba == aa.min(ac) + ta.chars().count()
            && !a.ctx.track
            && !b.ctx.track
            && a.ctx.author == b.ctx.author
            && ta.len() + tb.len() <= 4096;
        if joins {
            a.params = serde_json::json!({ "text": format!("{ta}{tb}") });
        }
        joins
    }
}

/// A presentation from its native file in base64 (a snapshot, a kept version).
pub fn decode(text: &str) -> Option<Presentation> {
    let bytes = deckcraft_engine::cmd::base64_decode(text)?;
    deckcraft_format::load(&bytes).ok()
}
