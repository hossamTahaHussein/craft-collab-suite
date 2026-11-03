//! What one revision changed in a presentation, as far as people's positions care: slides and
//! shapes that went (positions on them go to the neighbouring slide or leave the shape), and text
//! that changed in a shape or in a slide's notes (a caret in it moves as Docs' carets do: before
//! the change it stays, after it moves by the change's length, inside it goes to its end).
//! Slides and shapes keep their ids, so moving, resizing or reordering them moves nobody.

use deckcraft_model::{Presentation, ShapeId, Slide, SlideId, TextBody, walk};
use serde::{Deserialize, Serialize};

/// Which text: a shape's body or a slide's speaker notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextKey {
    Shape(ShapeId),
    Notes(SlideId),
}

/// One changed run of a text's linear form (paragraphs joined by `\n`), in characters: what was
/// `[start, old_end)` is `[start, new_end)` now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub start: usize,
    pub old_end: usize,
    pub new_end: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlideChange {
    /// `(slide, the slide now in its place)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_slides: Vec<(SlideId, SlideId)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_shapes: Vec<ShapeId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text: Vec<(TextKey, Span)>,
}

impl SlideChange {
    pub fn is_empty(&self) -> bool {
        self.removed_slides.is_empty() && self.removed_shapes.is_empty() && self.text.is_empty()
    }
}

/// A text body as one string: its paragraphs' runs, paragraphs joined by `\n`.
pub fn linear(body: &TextBody) -> String {
    body.paragraphs.iter().map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// `(paragraph, char)` in `body` as a linear offset, clamped.
pub fn offset_of(body: &TextBody, (para, ch): (usize, usize)) -> usize {
    let mut off = 0;
    for (i, p) in body.paragraphs.iter().enumerate() {
        let len: usize = p.runs.iter().map(|r| r.text.chars().count()).sum();
        if i == para {
            return off + ch.min(len);
        }
        off += len + 1;
    }
    off.saturating_sub(1)
}

/// A linear offset in `body` as `(paragraph, char)`, clamped.
pub fn pos_at(body: &TextBody, mut off: usize) -> (usize, usize) {
    let n = body.paragraphs.len();
    for (i, p) in body.paragraphs.iter().enumerate() {
        let len: usize = p.runs.iter().map(|r| r.text.chars().count()).sum();
        if off <= len || i + 1 == n {
            return (i, off.min(len));
        }
        off -= len + 1;
    }
    (0, 0)
}

/// The changed run between two texts: common prefix and suffix, in characters.
pub fn span(before: &str, after: &str) -> Option<Span> {
    if before == after {
        return None;
    }
    let a: Vec<char> = before.chars().collect();
    let b: Vec<char> = after.chars().collect();
    let p = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let s = a[p..].iter().rev().zip(b[p..].iter().rev()).take_while(|(x, y)| x == y).count();
    Some(Span { start: p, old_end: a.len() - s, new_end: b.len() - s })
}

/// Someone else's caret at `off` after `s` (theirs stays before an insertion at its place).
pub fn map_offset(off: usize, s: &Span) -> usize {
    if off <= s.start {
        off
    } else if off >= s.old_end {
        off + s.new_end - s.old_end
    } else {
        s.new_end
    }
}

/// Every text body of a slide, by key.
fn texts(slide: &Slide) -> Vec<(TextKey, &TextBody)> {
    let mut out = vec![(TextKey::Notes(slide.id), &slide.notes)];
    walk(&slide.shapes, &mut |sh, _| {
        if let Some(t) = &sh.text {
            out.push((TextKey::Shape(sh.id), t));
        }
    });
    out
}

fn shape_ids(slide: &Slide) -> Vec<ShapeId> {
    let mut out = Vec::new();
    walk(&slide.shapes, &mut |sh, _| out.push(sh.id));
    out
}

/// What changed from `before` to `after`.
pub fn changes(before: &Presentation, after: &Presentation) -> SlideChange {
    let mut out = SlideChange::default();
    for (i, s) in before.slides.iter().enumerate() {
        let Some(now) = after.slides.iter().find(|n| n.id == s.id) else {
            let next = after.slides.get(i.min(after.slides.len().saturating_sub(1))).map_or(SlideId(0), |n| n.id);
            out.removed_slides.push((s.id, next));
            continue;
        };
        if s == now {
            continue;
        }
        let kept = shape_ids(now);
        out.removed_shapes.extend(shape_ids(s).into_iter().filter(|id| !kept.contains(id)));
        let later = texts(now);
        for (key, body) in texts(s) {
            if let Some((_, new)) = later.iter().find(|(k, _)| *k == key)
                && let Some(sp) = span(&linear(body), &linear(new))
            {
                out.text.push((key, sp));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caret_moves_with_text_typed_before_it_and_not_after() {
        let s = span("Hello world", "Hello big world").unwrap();
        assert_eq!(s, Span { start: 6, old_end: 6, new_end: 10 });
        assert_eq!(map_offset(8, &s), 12, "after the insertion: moves by its length");
        assert_eq!(map_offset(6, &s), 6, "at it: someone else's caret stays before");
        assert_eq!(map_offset(2, &s), 2);
    }

    #[test]
    fn a_caret_inside_deleted_text_goes_to_its_end() {
        let s = span("abcdef", "af").unwrap();
        assert_eq!(map_offset(3, &s), 1);
        assert_eq!(map_offset(6, &s), 2);
    }

    #[test]
    fn offsets_and_positions_round_trip_across_paragraphs() {
        let mut body = TextBody { paragraphs: vec![Default::default(), Default::default()], ..Default::default() };
        body.paragraphs[0].runs = vec![deckcraft_model::Run::new("ab")];
        body.paragraphs[1].runs = vec![deckcraft_model::Run::new("cde")];
        assert_eq!(linear(&body), "ab\ncde");
        assert_eq!(offset_of(&body, (1, 2)), 5);
        assert_eq!(pos_at(&body, 5), (1, 2));
        assert_eq!(pos_at(&body, 2), (0, 2));
        assert_eq!(pos_at(&body, 3), (1, 0));
        assert_eq!(pos_at(&body, 99), (1, 3), "clamped");
    }
}
