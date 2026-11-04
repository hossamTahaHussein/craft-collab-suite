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
//! presentation's own settings (theme, size, masters) can't be suggested.

use std::sync::Arc;

use deckcraft_model::{Presentation, Shape, ShapeId, Slide, SlideId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cmd::{CommandSpec, bool_or, cmd, has_doc, str_param};
use crate::{EngineError, Result, Session};

/// The presentation tag holding the suggestions.
const TAG: &str = "CRAFT_SUGGESTIONS";

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
    /// A whole slide: as it was (none: it is new) and its place.
    Slide { slide: SlideId, index: usize, was: Option<Box<Slide>> },
    /// A slide's own settings (its shapes aside): layout, background, transition, notes…
    Settings { slide: SlideId, was: Box<Slide> },
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

/// The presentation's suggestions, oldest first.
pub fn suggestions(p: &Presentation) -> Vec<Suggestion> {
    p.tags.iter().find(|(k, _)| k == TAG).and_then(|(_, v)| serde_json::from_str(v).ok()).unwrap_or_default()
}

fn store(p: &mut Presentation, list: &[Suggestion]) {
    p.tags.retain(|(k, _)| k != TAG);
    if !list.is_empty()
        && let Ok(v) = serde_json::to_string(list)
    {
        p.tags.push((TAG.to_string(), v));
    }
}

/// `slide` without its shapes: its own settings.
fn settings(slide: &Slide) -> Slide {
    Slide { shapes: Vec::new(), ..slide.clone() }
}

/// Records, in `after`, `author`'s suggestion at `time` for each shape, slide and slide setting
/// that differs from `before`. What was suggested before keeps its earliest state; its author
/// becomes the latest. Moving slides, and changes outside the slides, can't be suggested.
pub(crate) fn record(before: &Presentation, after: &mut Presentation, author: &str, time: u64) -> Result<()> {
    let outside = |p: &Presentation| Presentation { slides: Vec::new(), tags: Vec::new(), next_id: 0, ..p.clone() };
    if outside(before) != outside(after) {
        return Err(EngineError::Other("Only changes to slides and shapes can be suggested".into()));
    }
    let kept: Vec<SlideId> = before.slides.iter().map(|s| s.id).filter(|id| after.slides.iter().any(|s| s.id == *id)).collect();
    let now: Vec<SlideId> = after.slides.iter().map(|s| s.id).filter(|id| kept.contains(id)).collect();
    if kept != now {
        return Err(EngineError::Other("Moving slides can't be suggested".into()));
    }
    let mut found = Vec::new();
    for (i, b) in before.slides.iter().enumerate() {
        match after.slides.iter().find(|s| s.id == b.id) {
            None => found.push(Before::Slide { slide: b.id, index: i, was: Some(Box::new((**b).clone())) }),
            Some(a) if Arc::ptr_eq(a, b) => {}
            Some(a) => {
                if settings(a) != settings(b) {
                    found.push(Before::Settings { slide: b.id, was: Box::new(settings(b)) });
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
        if !before.slides.iter().any(|s| s.id == a.id) {
            found.push(Before::Slide { slide: a.id, index: i, was: None });
        }
    }
    if found.is_empty() {
        return Ok(());
    }
    let mut list = suggestions(after);
    for before in found {
        let new = Suggestion { before, author: author.to_string(), time };
        match list.iter_mut().find(|x| x.target() == new.target()) {
            Some(x) => x.author = new.author,
            None => list.push(new),
        }
    }
    store(after, &list);
    Ok(())
}

/// Puts back what suggestion `x` changed in `p`.
fn put_back(p: &mut Presentation, x: &Suggestion) {
    match &x.before {
        Before::Slide { slide, index, was } => match was {
            None => p.slides.retain(|s| s.id != *slide),
            Some(s) if !p.slides.iter().any(|t| t.id == *slide) => {
                let at = (*index).min(p.slides.len());
                p.slides.insert(at, Arc::new((**s).clone()));
            }
            Some(s) => {
                if let Some(t) = p.slides.iter_mut().find(|t| t.id == *slide) {
                    *t = Arc::new((**s).clone());
                }
            }
        },
        Before::Settings { slide, was } => {
            if let Some(t) = p.slides.iter_mut().find(|t| t.id == *slide) {
                let shapes = std::mem::take(&mut Arc::make_mut(t).shapes);
                *t = Arc::new(Slide { shapes, ..(**was).clone() });
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

/// The suggestions a review command reaches: those of the selected shapes (else the slide shown)
/// for one; everyone's, or `author`'s, for all.
fn chosen(s: &Session, p: &Value, all: bool) -> Result<Vec<Suggestion>> {
    let d = s.doc()?;
    let list = suggestions(&d.doc);
    if all {
        let author = str_param(p, "author");
        return Ok(list.into_iter().filter(|x| author.is_none_or(|a| x.author == a)).collect());
    }
    let slide = d.doc.slides.get(d.selection.slide).map(|s| s.id);
    let shapes = &d.selection.shapes;
    let on_slide: Vec<Suggestion> = list.into_iter().filter(|x| Some(x.slide()) == slide).collect();
    let of_shapes: Vec<Suggestion> = on_slide.iter().filter(|x| x.shape().is_some_and(|id| shapes.contains(&id))).cloned().collect();
    Ok(if of_shapes.is_empty() { on_slide } else { of_shapes })
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
            "{} (the selected shapes', else the slide's) → {count}",
            has_doc,
            |s, p| review(s, p, false, true)
        ),
        cmd!(
            "review.rejectSuggestion",
            "Reject Suggestion",
            ["Review", "Suggesting"],
            None,
            "{} (the selected shapes', else the slide's) → {count}",
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
