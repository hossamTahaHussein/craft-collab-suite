//! Suggesting: edits made as suggestions, which their author or anyone who may edit accepts or
//! rejects, shape by shape, slide by slide, person by person or all at once (as Craft Docs'
//! Suggesting mode).
//!
//! A suggested edit shows at once; what it changed is kept with who suggested it and when: a
//! shape's earlier state (none for a new shape), a slide's (none for a new slide, the slide and
//! its place for a deleted one) or a slide's own settings (layout, background, transition,
//! notes). Accepting drops the record; rejecting puts the earlier state back (rejecting a new
//! shape or slide deletes it). The records are kept in the presentation (a presentation tag,
//! saved in .pptx and .deckcraft), so they travel in files, co-editing snapshots and undo steps,
//! and every replica records the same ones: an edit replayed with `track` on its context records
//! them as it runs.
//!
//! Only edits of slides, shapes and their text can be suggested ([`SUGGEST_COMMANDS`]): one Reject
//! All test per command proves the presentation comes back as it was. Moving slides and the
//! presentation's own settings (theme, size, masters) can't be suggested. A slide added or deleted
//! as a suggestion keeps its place in sections and custom shows when rejected.
//!
//! A direct edit (not a suggestion) of a suggested shape or slide setting settles that
//! suggestion: the shape is what that person made it, and rejecting won't take their edit back.
//! Anyone who may edit reviews; a suggester can't, as in Craft Docs.

use std::sync::Arc;

use deckcraft_model::{Presentation, Shape, ShapeId, Slide, SlideId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cmd::{CommandSpec, bool_or, cmd, has_doc, str_param};
use crate::{EngineError, Result, Session};

/// The presentation tag holding the suggestions.
pub(crate) const TAG: &str = "CRAFT_SUGGESTIONS";

/// The commands an edit may be suggested with. The relay's `suggest-commands.slides.json` lists
/// the same.
pub const SUGGEST_COMMANDS: &[&str] = &[
    "text.insert",
    "text.delete",
    "text.set",
    "format.bold",
    "format.italic",
    "format.underline",
    "format.strikethrough",
    "format.superscript",
    "format.subscript",
    "format.font",
    "format.size",
    "format.grow",
    "format.shrink",
    "format.color",
    "format.highlight",
    "format.case",
    "format.clear",
    "format.alignLeft",
    "format.alignCenter",
    "format.alignRight",
    "format.justify",
    "format.bullets",
    "format.numbering",
    "format.indent",
    "format.outdent",
    "format.rtl",
    "shape.insert",
    "insert.textBox",
    "shape.move",
    "shape.resize",
    "shape.setBounds",
    "shape.rotate",
    "shape.flip",
    "shape.fill",
    "shape.line",
    "edit.delete",
    "edit.duplicate",
    "slide.new",
    "slide.duplicate",
    "slide.delete",
    "slide.hide",
    "slide.layout",
    // Typing through an input method (CJK, dead keys, press-and-hold).
    "text.compose",
    "text.commit",
    "text.deleteSurrounding",
];

/// The commands that work on suggestions themselves (never suggested).
pub const REVIEW_COMMANDS: &[&str] =
    &["review.acceptSuggestion", "review.rejectSuggestion", "review.acceptAllSuggestions", "review.rejectAllSuggestions"];

/// What a suggestion changed, and how it was before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Before {
    /// A shape on slide `slide`: as it was (none: it is new) and where it was among the slide's
    /// shapes.
    Shape { slide: SlideId, shape: ShapeId, index: usize, was: Option<Box<Shape>> },
    /// A whole slide: as it was (none: it is new), its place, and for a deleted one where it was
    /// in sections and custom shows (by name, at that position).
    Slide {
        slide: SlideId,
        index: usize,
        was: Option<Box<Slide>>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        places: Vec<(String, usize, bool)>,
    },
    /// A slide's own settings (its shapes and comments aside), as they were: only those the
    /// suggestions changed (layout, background, transition, notes…), by their names in the file.
    Settings { slide: SlideId, was: serde_json::Map<String, Value> },
}

/// One suggested change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub before: Before,
    pub author: String,
    /// Seconds since the Unix epoch.
    pub time: u64,
}

impl Suggestion {
    /// The slide it is on.
    pub fn slide(&self) -> SlideId {
        match &self.before {
            Before::Shape { slide, .. } | Before::Slide { slide, .. } | Before::Settings { slide, .. } => *slide,
        }
    }

    /// The shape it is about, for a shape's.
    pub fn shape(&self) -> Option<ShapeId> {
        match &self.before {
            Before::Shape { shape, .. } => Some(*shape),
            _ => None,
        }
    }

    /// What it is about, to find an earlier record of the same thing.
    fn target(&self) -> (u8, SlideId, Option<ShapeId>) {
        match &self.before {
            Before::Shape { slide, shape, .. } => (0, *slide, Some(*shape)),
            Before::Slide { slide, .. } => (1, *slide, None),
            Before::Settings { slide, .. } => (2, *slide, None),
        }
    }
}

/// The presentation's suggestions, oldest first. A record this version can't read (written by a
/// newer one) is left out here and kept in the presentation ([`store`]).
pub fn suggestions(p: &Presentation) -> Vec<Suggestion> {
    raw(p).into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect()
}

fn raw(p: &Presentation) -> Vec<Value> {
    match p.tags.iter().find(|(k, _)| k == TAG).and_then(|(_, v)| serde_json::from_str(v).ok()) {
        Some(Value::Array(v)) => v,
        _ => Vec::new(),
    }
}

/// [`suggestions`] for painting, read again only when the records change: the front end asks
/// every frame, for every thumbnail. Keyed on the stored text (where it is and how it ends),
/// which any change of the records replaces.
pub fn cached(p: &Presentation) -> Arc<Vec<Suggestion>> {
    thread_local! {
        static CACHE: std::cell::RefCell<Option<(u64, Arc<Vec<Suggestion>>)>> = const { std::cell::RefCell::new(None) };
    }
    let Some((_, text)) = p.tags.iter().find(|(k, _)| k == TAG) else { return Arc::new(Vec::new()) };
    let mut key: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        key ^= v;
        key = key.wrapping_mul(0x0000_0100_0000_01b3);
    };
    mix(text.as_ptr() as u64);
    mix(text.len() as u64);
    for b in text.as_bytes().get(text.len().saturating_sub(256)..).unwrap_or_default() {
        mix(u64::from(*b));
    }
    CACHE.with(|c| {
        if let Some((k, list)) = &*c.borrow()
            && *k == key
        {
            return list.clone();
        }
        let list = Arc::new(suggestions(p));
        *c.borrow_mut() = Some((key, list.clone()));
        list
    })
}

fn store(p: &mut Presentation, list: &[Suggestion]) {
    let mut all: Vec<Value> = raw(p).into_iter().filter(|v| serde_json::from_value::<Suggestion>(v.clone()).is_err()).collect();
    all.extend(list.iter().filter_map(|x| serde_json::to_value(x).ok()));
    p.tags.retain(|(k, _)| k != TAG);
    if !all.is_empty()
        && let Ok(v) = serde_json::to_string(&all)
    {
        p.tags.push((TAG.to_string(), v));
    }
}

/// `slide`'s own settings by their names in the file: all but its shapes and comments.
fn settings(slide: &Slide) -> serde_json::Map<String, Value> {
    match serde_json::to_value(Slide { shapes: Vec::new(), comments: Vec::new(), ..slide.clone() }) {
        Ok(Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    }
}

/// The settings that differ between two versions of a slide, as they were in `b`.
fn changed_settings(b: &Slide, a: &Slide) -> serde_json::Map<String, Value> {
    let (sb, sa) = (settings(b), settings(a));
    let mut keys: Vec<&String> = sb.keys().chain(sa.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter().filter(|k| sb.get(*k) != sa.get(*k)).map(|k| (k.clone(), sb.get(k).cloned().unwrap_or(Value::Null))).collect()
}

/// Where slide `id` is in sections and custom shows: (name, position, a custom show?).
fn places(p: &Presentation, id: SlideId) -> Vec<(String, usize, bool)> {
    let sections = p.sections.iter().filter_map(|s| Some((s.name.clone(), s.slides.iter().position(|x| *x == id)?, false)));
    let shows = p.custom_shows.iter().filter_map(|s| Some((s.name.clone(), s.slides.iter().position(|x| *x == id)?, true)));
    sections.chain(shows).collect()
}

/// Sections and custom shows without the slides in `gone`.
fn lists_without(p: &Presentation, gone: &[SlideId]) -> (Vec<(String, Vec<SlideId>)>, Vec<(String, Vec<SlideId>)>) {
    let strip = |name: &String, slides: &Vec<SlideId>| (name.clone(), slides.iter().copied().filter(|x| !gone.contains(x)).collect());
    (p.sections.iter().map(|s| strip(&s.name, &s.slides)).collect(), p.custom_shows.iter().map(|s| strip(&s.name, &s.slides)).collect())
}

/// Records, in `after`, `author`'s suggestion at `time` for each shape, slide and slide setting
/// that differs from `before`. What was suggested before keeps its earliest state (a setting
/// suggested again keeps its first value too); its author becomes the latest. A shape back as it
/// was (a composition cancelled) is no longer a suggestion. Moving slides, and changes outside
/// the slides (but for sections and custom shows following slides added or deleted), can't be
/// suggested.
pub(crate) fn record(before: &Presentation, after: &mut Presentation, author: &str, time: u64) -> Result<()> {
    let only = || EngineError::Other("Only changes to slides and shapes can be suggested".into());
    let outside = |p: &Presentation| Presentation {
        slides: Vec::new(),
        tags: Vec::new(),
        next_id: 0,
        sections: Vec::new(),
        custom_shows: Vec::new(),
        ..p.clone()
    };
    if outside(before) != outside(after) {
        return Err(only());
    }
    let added: Vec<SlideId> = after.slides.iter().map(|s| s.id).filter(|id| !before.slides.iter().any(|s| s.id == *id)).collect();
    let deleted: Vec<SlideId> = before.slides.iter().map(|s| s.id).filter(|id| !after.slides.iter().any(|s| s.id == *id)).collect();
    let changed: Vec<SlideId> = added.iter().chain(&deleted).copied().collect();
    if lists_without(before, &changed) != lists_without(after, &changed) {
        return Err(only());
    }
    let kept: Vec<SlideId> = before.slides.iter().map(|s| s.id).filter(|id| after.slides.iter().any(|s| s.id == *id)).collect();
    let now: Vec<SlideId> = after.slides.iter().map(|s| s.id).filter(|id| kept.contains(id)).collect();
    if kept != now {
        return Err(EngineError::Other("Moving slides can't be suggested".into()));
    }
    let mut found = Vec::new();
    for (i, b) in before.slides.iter().enumerate() {
        match after.slides.iter().find(|s| s.id == b.id) {
            None => found.push(Before::Slide { slide: b.id, index: i, was: Some(Box::new((**b).clone())), places: places(before, b.id) }),
            Some(a) if Arc::ptr_eq(a, b) => {}
            Some(a) => {
                let was = changed_settings(b, a);
                if !was.is_empty() {
                    found.push(Before::Settings { slide: b.id, was });
                }
                for (k, sb) in b.shapes.iter().enumerate() {
                    match a.shapes.iter().find(|x| x.id == sb.id) {
                        Some(sa) if sa == sb => {}
                        _ => found.push(Before::Shape { slide: b.id, shape: sb.id, index: k, was: Some(Box::new(sb.clone())) }),
                    }
                }
                for (k, sa) in a.shapes.iter().enumerate() {
                    if !b.shapes.iter().any(|x| x.id == sa.id) {
                        found.push(Before::Shape { slide: a.id, shape: sa.id, index: k, was: None });
                    }
                }
            }
        }
    }
    for (i, a) in after.slides.iter().enumerate() {
        if added.contains(&a.id) {
            found.push(Before::Slide { slide: a.id, index: i, was: None, places: Vec::new() });
        }
    }
    // The records as they were before the edit: an edit that puts back a snapshot (a cancelled
    // composition) would take them with it.
    after.tags.retain(|(k, _)| k != TAG);
    after.tags.extend(before.tags.iter().filter(|(k, _)| k == TAG).cloned());
    if found.is_empty() {
        return Ok(());
    }
    let mut list = suggestions(after);
    for before in found {
        let new = Suggestion { before, author: author.to_string(), time };
        match list.iter_mut().find(|x| x.target() == new.target()) {
            Some(x) => {
                x.author = new.author;
                // A setting suggested for the first time keeps its value from before this edit.
                if let (Before::Settings { was: old, .. }, Before::Settings { was: more, .. }) = (&mut x.before, new.before) {
                    for (k, v) in more {
                        old.entry(k).or_insert(v);
                    }
                }
            }
            None => list.push(new),
        }
    }
    // A shape back as it was, or settings all back, suggests nothing.
    list.retain(|x| match &x.before {
        // By their saved form: a record's shape has been through it.
        Before::Shape { slide, shape, was: Some(was), .. } => {
            let now = after.slides.iter().find(|s| s.id == *slide).and_then(|s| s.shapes.iter().find(|sh| sh.id == *shape));
            now.and_then(|n| serde_json::to_value(n).ok()) != serde_json::to_value(&**was).ok()
        }
        Before::Settings { slide, was } => after.slides.iter().find(|s| s.id == *slide).is_none_or(|s| {
            let now = settings(s);
            was.iter().any(|(k, v)| now.get(k).unwrap_or(&Value::Null) != v)
        }),
        _ => true,
    });
    store(after, &list);
    Ok(())
}

/// The records after a direct edit (not a suggestion) from `before` to `after`: a shape it
/// changed, a slide setting it changed and a slide it deleted are settled, as that person made
/// them. Whether anything changed.
pub(crate) fn follow(before: &Presentation, after: &mut Presentation) -> bool {
    let list = suggestions(after);
    let n = list.len();
    let slide = |p: &Presentation, id: SlideId| p.slides.iter().find(|s| s.id == id).cloned();
    let kept: Vec<Suggestion> = list
        .into_iter()
        .filter(|x| {
            let (b, a) = (slide(before, x.slide()), slide(after, x.slide()));
            match &x.before {
                Before::Shape { shape, .. } => {
                    let find = |s: &Option<Arc<Slide>>| s.as_ref().and_then(|s| s.shapes.iter().find(|sh| sh.id == *shape).cloned());
                    find(&b) == find(&a)
                }
                Before::Settings { was, .. } => match (b, a) {
                    (Some(b), Some(a)) => {
                        let (sb, sa) = (settings(&b), settings(&a));
                        !was.keys().any(|k| sb.get(k) != sa.get(k))
                    }
                    _ => false,
                },
                // A slide suggested new and deleted directly: settled.
                Before::Slide { was: None, .. } => a.is_some() || b.is_none(),
                Before::Slide { .. } => true,
            }
        })
        .collect();
    if kept.len() == n {
        return false;
    }
    store(after, &kept);
    true
}

/// Puts back what suggestion `x` changed in `p`.
fn put_back(p: &mut Presentation, x: &Suggestion) {
    match &x.before {
        Before::Slide { slide, index, was, places } => match was {
            None => {
                p.slides.retain(|s| s.id != *slide);
                for list in p.sections.iter_mut().map(|s| &mut s.slides).chain(p.custom_shows.iter_mut().map(|s| &mut s.slides)) {
                    list.retain(|x| x != slide);
                }
            }
            Some(s) if !p.slides.iter().any(|t| t.id == *slide) => {
                let at = (*index).min(p.slides.len());
                p.slides.insert(at, Arc::new((**s).clone()));
                for (name, at, show) in places {
                    let list = if *show {
                        p.custom_shows.iter_mut().find(|s| s.name == *name).map(|s| &mut s.slides)
                    } else {
                        p.sections.iter_mut().find(|s| s.name == *name).map(|s| &mut s.slides)
                    };
                    if let Some(list) = list
                        && !list.contains(slide)
                    {
                        list.insert((*at).min(list.len()), *slide);
                    }
                }
            }
            Some(s) => {
                if let Some(t) = p.slides.iter_mut().find(|t| t.id == *slide) {
                    *t = Arc::new((**s).clone());
                }
            }
        },
        Before::Settings { slide, was } => {
            if let Some(t) = p.slides.iter_mut().find(|t| t.id == *slide) {
                let mut now = settings(t);
                for (k, v) in was {
                    now.insert(k.clone(), v.clone());
                }
                if let Ok(back) = serde_json::from_value::<Slide>(Value::Object(now)) {
                    let t = Arc::make_mut(t);
                    *t = Slide { shapes: std::mem::take(&mut t.shapes), comments: std::mem::take(&mut t.comments), ..back };
                }
            }
        }
        Before::Shape { slide, shape, index, was } => {
            let Some(t) = p.slides.iter_mut().find(|t| t.id == *slide) else { return };
            let t = Arc::make_mut(t);
            match (was, t.shapes.iter().position(|s| s.id == *shape)) {
                (None, Some(i)) => {
                    t.shapes.remove(i);
                }
                (Some(s), Some(i)) => t.shapes[i] = (**s).clone(),
                (Some(s), None) => {
                    let at = (*index).min(t.shapes.len());
                    t.shapes.insert(at, (**s).clone());
                }
                (None, None) => {}
            }
        }
    }
}

/// The suggestions Accept and Reject reach in `doc` with `sel`: the selected shapes', or with no
/// shape selected, the slide shown's.
pub fn reachable(doc: &Presentation, sel: &crate::Selection, list: &[Suggestion]) -> Vec<Suggestion> {
    let slide = doc.slides.get(sel.slide).map(|s| s.id);
    let on_slide = list.iter().filter(|x| Some(x.slide()) == slide);
    if sel.shapes.is_empty() {
        on_slide.cloned().collect()
    } else {
        on_slide.filter(|x| x.shape().is_some_and(|id| sel.shapes.contains(&id))).cloned().collect()
    }
}

/// The suggestions a review command reaches: [`reachable`] for one; everyone's, or `author`'s,
/// for all.
fn chosen(s: &Session, p: &Value, all: bool) -> Result<Vec<Suggestion>> {
    let d = s.doc()?;
    let list = suggestions(&d.doc);
    if all {
        let author = str_param(p, "author");
        return Ok(list.into_iter().filter(|x| author.is_none_or(|a| x.author == a)).collect());
    }
    Ok(reachable(&d.doc, &d.selection, &list))
}

/// Accepts (`keep`) or rejects the suggestions `p` names, newest first when putting them back.
fn review(s: &mut Session, p: &Value, all: bool, keep: bool) -> Result<Value> {
    let chosen = chosen(s, p, all)?;
    if chosen.is_empty() {
        return Ok(json!({"count": 0}));
    }
    let count = chosen.len();
    s.edit(|doc, sel| {
        if !keep {
            for x in chosen.iter().rev() {
                put_back(doc, x);
            }
            sel.slide = sel.slide.min(doc.slides.len().saturating_sub(1));
            // The text being edited may be gone or put back: editing stops.
            sel.text = None;
            sel.shapes.retain(|id| doc.slides.get(sel.slide).is_some_and(|sl| sl.shapes.iter().any(|x| x.id == *id)));
        }
        let rest: Vec<Suggestion> = suggestions(doc).into_iter().filter(|x| !chosen.contains(x)).collect();
        store(doc, &rest);
        Ok(json!({"count": count}))
    })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "review.suggesting", "Suggesting", ["Review", "Suggesting"], None, "{on?: bool (default: toggle)} → {on}", has_doc, |s, p| {
            s.suggesting = bool_or(p, "on", !s.suggesting);
            Ok(json!({"on": s.suggesting}))
        }),
        cmd!(query "review.suggestions", "Suggestions", ["Review", "Suggesting"], None, "{} → {suggestions: [{slide, shape?, kind, author, time}]}", has_doc, |s, _| {
            let d = s.doc()?;
            let list: Vec<Value> = suggestions(&d.doc)
                .iter()
                .map(|x| {
                    let index = d.doc.slides.iter().position(|sl| sl.id == x.slide());
                    let kind = match &x.before {
                        Before::Shape { was: None, .. } => "newShape",
                        Before::Shape { .. } if x.shape().is_some_and(|id| d.doc.slides.iter().all(|sl| sl.shapes.iter().all(|sh| sh.id != id))) => "deletedShape",
                        Before::Shape { .. } => "shape",
                        Before::Slide { was: None, .. } => "newSlide",
                        Before::Slide { .. } => "deletedSlide",
                        Before::Settings { .. } => "slide",
                    };
                    json!({"slide": index, "slideId": x.slide().0, "shape": x.shape().map(|id| id.0), "kind": kind, "author": x.author, "time": x.time})
                })
                .collect();
            Ok(json!({"suggestions": list}))
        }),
        cmd!(
            "review.acceptSuggestion",
            "Accept Suggestion",
            ["Review", "Suggesting"],
            None,
            "{} (the selected shapes', or with none selected the slide's) → {count}",
            has_doc,
            |s, p| review(s, p, false, true)
        ),
        cmd!(
            "review.rejectSuggestion",
            "Reject Suggestion",
            ["Review", "Suggesting"],
            None,
            "{} (the selected shapes', or with none selected the slide's) → {count}",
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
#[path = "tests_suggest.rs"]
mod tests;
