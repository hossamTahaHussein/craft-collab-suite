//! Where an edit happened, and where a position moves to because of it (spec §5). A story's
//! linear text (`wordcraft_doc::offsets`) is compared before and after: what the two share at
//! the start and at the end didn't change, and the bytes in between did.

use std::collections::BTreeSet;
use std::sync::Arc;

use wordcraft_doc::offsets::{blocks_len, blocks_text, linear_text};
use wordcraft_doc::{Blocks, Document, StoryRef};

/// The changed part of a text: `start..old_end` before became `start..new_end` after. All three
/// are char boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub old_end: usize,
    pub new_end: usize,
}

/// The changed part of `before` → `after`; `None` when they are the same.
pub fn span(before: &str, after: &str) -> Option<Span> {
    if before == after {
        return None;
    }
    let (a, b) = (before.as_bytes(), after.as_bytes());
    let mut start = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    // The shared start is identical in both texts, so a boundary in one is one in the other.
    while start > 0 && !before.is_char_boundary(start) {
        start -= 1;
    }
    let room = a.len().min(b.len()) - start;
    let suffix = a.iter().rev().zip(b.iter().rev()).take(room).take_while(|(x, y)| x == y).count();
    let mut old_end = a.len() - suffix;
    let mut new_end = b.len() - suffix;
    // Grow the ends to whole characters; the shared tail grows identically in both.
    while !before.is_char_boundary(old_end) || !after.is_char_boundary(new_end) {
        old_end += 1;
        new_end += 1;
    }
    Some(Span { start, old_end, new_end })
}

/// [`span`], placed at `hint` when the change could be there too. Inserting or deleting text that
/// repeats its neighbours ("o" before the "o" of "Hello") reads the same at several places, and
/// the shared-start rule picks the last; `hint` (where the edit's selection started) picks the
/// one that happened.
pub fn span_near(before: &str, after: &str, hint: usize) -> Option<Span> {
    let s = span(before, after)?;
    if hint >= s.start {
        return Some(s);
    }
    let (old_end, new_end) = (hint + (s.old_end - s.start), hint + (s.new_end - s.start));
    let fits = before.is_char_boundary(hint)
        && after.is_char_boundary(hint)
        && before.is_char_boundary(old_end)
        && after.is_char_boundary(new_end)
        && before.as_bytes().get(..hint) == after.as_bytes().get(..hint)
        && before.as_bytes().get(old_end..) == after.as_bytes().get(new_end..);
    Some(if fits { Span { start: hint, old_end, new_end } } else { s })
}

/// Where `offset` (in the text before) is after the change. The author's own caret at the
/// change goes after what they inserted; everyone else's stays before it.
pub fn map(span: &Span, offset: usize, is_author_caret: bool) -> usize {
    if offset < span.start || (offset == span.start && !is_author_caret) {
        offset
    } else if offset >= span.old_end && !(offset == span.start && is_author_caret) {
        offset - span.old_end + span.new_end
    } else {
        span.new_end
    }
}

/// The stories whose content differs between two versions of a document, found by comparing
/// the shared block pointers (an edit copies only the blocks it touches).
pub fn changed_stories(before: &Document, after: &Document) -> Vec<StoryRef> {
    let mut out = Vec::new();
    if !same_blocks(&before.body, &after.body) {
        out.push(StoryRef::Body);
    }
    let ids: BTreeSet<u32> = before.parts.keys().chain(after.parts.keys()).copied().collect();
    for id in ids {
        let same = match (before.parts.get(&id), after.parts.get(&id)) {
            (Some(a), Some(b)) => same_blocks(&a.blocks, &b.blocks),
            _ => false,
        };
        if !same {
            out.push(StoryRef::Part(id));
        }
    }
    out
}

fn same_blocks(a: &Blocks, b: &Blocks) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| Arc::ptr_eq(x, y) || x == y)
}

/// What changed in each story; in the story of `hint` (the edit's selection start), the change is
/// placed there when it could be (`mapping::span_near`).
pub fn changes(before: &Document, after: &Document, hint: Option<(StoryRef, usize)>) -> Vec<(StoryRef, Span)> {
    changed_stories(before, after).into_iter().filter_map(|story| story_diff(before, after, story, hint).map(|s| (story, s))).collect()
}

/// [`diff_near`] of the story's linear texts in `before` and `after`. The blocks the two share
/// at the start and the end (an edit copies only the blocks it touches) are measured, not
/// written out and compared: only the blocks between are. Where that could give another answer
/// (the change reaches the shared blocks, or `hint` sits before it) the whole texts are compared.
pub fn story_diff(before: &Document, after: &Document, story: StoryRef, hint: Option<(StoryRef, usize)>) -> Option<Span> {
    if let Some(exact) = middle_diff(before, after, story, hint) {
        return exact;
    }
    diff_near(&linear_text(before, story), &linear_text(after, story), story, hint)
}

/// [`story_diff`] from the blocks between the shared ones; `None` when it must compare the
/// whole texts.
fn middle_diff(before: &Document, after: &Document, story: StoryRef, hint: Option<(StoryRef, usize)>) -> Option<Option<Span>> {
    let (a, b) = (before.story(story)?, after.story(story)?);
    let lead_blocks = a.iter().zip(b).take_while(|(x, y)| Arc::ptr_eq(x, y)).count();
    let room = a.len().min(b.len()) - lead_blocks;
    let tail_blocks = a.iter().rev().zip(b.iter().rev()).take(room).take_while(|(x, y)| Arc::ptr_eq(x, y)).count();
    if lead_blocks + tail_blocks == 0 {
        return None;
    }
    let (am, bm) = (blocks_text(&a[lead_blocks..a.len() - tail_blocks]), blocks_text(&b[lead_blocks..b.len() - tail_blocks]));
    if am == bm {
        return Some(None);
    }
    let (lead, tail) = (blocks_len(&a[..lead_blocks]), blocks_len(&a[a.len() - tail_blocks..]));
    let (x, y) = (am.as_bytes(), bm.as_bytes());
    let shorter = x.len().min(y.len());
    // As `span` on the whole texts: the shared start, then the shared end in the room left.
    let prefix = x.iter().zip(y).take_while(|(p, q)| p == q).count();
    if prefix == shorter {
        return None;
    }
    let mut start = prefix;
    while start > 0 && !am.is_char_boundary(start) {
        start -= 1;
    }
    let suffix = x.iter().rev().zip(y.iter().rev()).take_while(|(p, q)| p == q).count();
    if suffix >= shorter || tail + suffix >= shorter + tail - start {
        return None;
    }
    let (mut old_end, mut new_end) = (x.len() - suffix, y.len() - suffix);
    while !am.is_char_boundary(old_end) || !bm.is_char_boundary(new_end) {
        old_end += 1;
        new_end += 1;
    }
    let s = Span { start: lead + start, old_end: lead + old_end, new_end: lead + new_end };
    match hint {
        Some((hs, h)) if hs == story && h < s.start => None,
        _ => Some(Some(s)),
    }
}

pub fn diff_near(before: &str, after: &str, story: StoryRef, hint: Option<(StoryRef, usize)>) -> Option<Span> {
    match hint {
        Some((hs, h)) if hs == story => span_near(before, after, h),
        _ => span(before, after),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use wordcraft_doc::{Block, Document, Paragraph, PartKind, StoryRef};

    use super::*;

    fn sp(before: &str, after: &str) -> Span {
        span(before, after).unwrap()
    }

    #[test]
    fn identical_text_has_no_span() {
        assert_eq!(span("abc", "abc"), None);
        assert_eq!(span("", ""), None);
    }

    #[test]
    fn insertion_span() {
        assert_eq!(sp("hello world", "hello big world"), Span { start: 6, old_end: 6, new_end: 10 });
    }

    #[test]
    fn deletion_span() {
        assert_eq!(sp("hello big world", "hello world"), Span { start: 6, old_end: 10, new_end: 6 });
    }

    #[test]
    fn repeated_letters_never_overlap_prefix_and_suffix() {
        // "aaa" → "aaaa": the prefix takes 3, the suffix may not take more than what's left.
        let s = sp("aaa", "aaaa");
        assert_eq!((s.start, s.old_end, s.new_end), (3, 3, 4));
    }

    #[test]
    fn span_near_places_an_ambiguous_insert_at_the_hint() {
        assert_eq!(span_near("Hello", "Helloo", 4), Some(Span { start: 4, old_end: 4, new_end: 5 }));
        assert_eq!(span_near("Hello", "Helloo", 5), Some(Span { start: 5, old_end: 5, new_end: 6 }));
        // A hint where the change can't be keeps the plain span.
        assert_eq!(span_near("Hello", "Helloo", 1), Some(Span { start: 5, old_end: 5, new_end: 6 }));
        assert_eq!(span_near("aaa", "aa", 0), Some(Span { start: 0, old_end: 1, new_end: 0 }));
    }

    #[test]
    fn mapping_table() {
        // "hello world" → "hello big world": insert "big " at 6.
        let ins = sp("hello world", "hello big world");
        let cases = [(0, false, 0), (5, false, 5), (6, false, 6), (6, true, 10), (7, false, 11), (11, false, 15)];
        for (offset, author, want) in cases {
            assert_eq!(map(&ins, offset, author), want, "insert: {offset} author={author}");
        }
        // "hello big world" → "hello world": delete "big " (6..10).
        let del = sp("hello big world", "hello world");
        for (offset, want) in [(5, 5), (6, 6), (8, 6), (10, 6), (15, 11)] {
            assert_eq!(map(&del, offset, false), want, "delete: {offset}");
        }
        // Replace "big" with "tiny": an offset inside the old text goes to the end of the new.
        let rep = sp("a big dog", "a tiny dog");
        assert_eq!(map(&rep, 3, false), 6);
        assert_eq!(map(&rep, 9, false), 10);
    }

    #[test]
    fn map_never_splits_chars() {
        let before = "مرحبا 👋 world";
        let after = "مرحبا 👍👋 world";
        let s = sp(before, after);
        assert!(before.is_char_boundary(s.start) && after.is_char_boundary(s.start));
        assert!(before.is_char_boundary(s.old_end) && after.is_char_boundary(s.new_end));
        for offset in (0..=before.len()).filter(|o| before.is_char_boundary(*o)) {
            for author in [false, true] {
                let m = map(&s, offset, author);
                assert!(after.is_char_boundary(m), "{offset} → {m}");
            }
        }
    }

    #[test]
    fn emoji_sharing_leading_bytes_is_one_change() {
        // 😀 F0 9F 98 80 and 😁 F0 9F 98 81 share three bytes: the span covers the whole char.
        let s = sp("a😀b", "a😁b");
        assert_eq!((s.start, s.old_end, s.new_end), (1, 5, 5));
    }

    #[test]
    fn a_megabyte_with_one_change_is_fast() {
        let before = "x".repeat(1 << 20);
        let mut after = before.clone();
        after.insert(1 << 19, 'y');
        let t = std::time::Instant::now();
        let s = sp(&before, &after);
        assert_eq!(s.start, 1 << 19);
        assert!(t.elapsed().as_millis() < 10, "{:?}", t.elapsed());
    }

    fn para(s: &str) -> Arc<Block> {
        Arc::new(Block::Para(Paragraph::with_text(s, Default::default())))
    }

    /// Documents for [`story_diff`]: paragraphs of a few repeated letters (so changes are often
    /// ambiguous) and a table, an edit's copy sharing the blocks it didn't touch.
    fn edited(texts: &[String], table_at: usize, edit: (u8, usize, usize, String)) -> (Document, Document) {
        let mut a = Document::new();
        a.body = texts.iter().map(|t| para(t)).collect();
        let mut t = wordcraft_doc::Table::new(1, 2, 200.0);
        t.rows[0].cells[0] = wordcraft_doc::table::Cell::with_text("ab");
        t.rows[0].cells[1] = wordcraft_doc::table::Cell::with_text("ba");
        a.body.insert(table_at.min(a.body.len()), Arc::new(Block::Table(t)));
        let mut b = a.clone();
        let (kind, i, at, text) = edit;
        let i = i % b.body.len();
        match kind % 4 {
            // Type into or delete from paragraph i.
            0 | 1 => {
                if let Block::Para(p) = &*b.body[i] {
                    let mut s = p.text.clone();
                    let at = at % (s.len() + 1);
                    if kind % 4 == 0 {
                        s.insert_str(at, &text);
                    } else {
                        s.replace_range(at..(at + text.len()).min(s.len()), "");
                    }
                    b.body[i] = para(&s);
                }
            }
            2 => b.body.insert(i, para(&text)),
            _ => {
                if b.body.len() > 1 {
                    b.body.remove(i);
                }
            }
        }
        (a, b)
    }

    proptest::proptest! {
        /// The fast comparison gives exactly what comparing the whole linear texts gives: every
        /// replica must place an edit the same way, whichever it ran.
        #[test]
        fn story_diff_matches_comparing_the_whole_texts(
            texts in proptest::collection::vec("[ab]{0,6}", 1..8),
            table_at in 0usize..9,
            edit in (0u8..4, 0usize..9, 0usize..8, "[ab]{1,3}"),
            hint in proptest::option::of(0usize..80),
        ) {
            let (a, b) = edited(&texts, table_at, edit);
            let hint = hint.map(|h| (StoryRef::Body, h));
            let whole = diff_near(&linear_text(&a, StoryRef::Body), &linear_text(&b, StoryRef::Body), StoryRef::Body, hint);
            proptest::prop_assert_eq!(story_diff(&a, &b, StoryRef::Body, hint), whole);
        }
    }

    #[test]
    fn changed_stories_finds_only_what_changed() {
        let mut a = Document::new();
        a.body = vec![para("one"), para("two")];
        let h = a.add_part(PartKind::Header, vec![para("head")]);
        let mut b = a.clone();
        assert!(changed_stories(&a, &b).is_empty());
        b.body[1] = para("TWO");
        assert_eq!(changed_stories(&a, &b), [StoryRef::Body]);
        let mut c = a.clone();
        if let Some(part) = c.parts.get_mut(&h) {
            part.blocks = vec![para("HEAD")];
        }
        assert_eq!(changed_stories(&a, &c), [StoryRef::Part(h)]);
        let mut d = a.clone();
        let f = d.add_part(PartKind::Footnote, vec![para("note")]);
        assert_eq!(changed_stories(&a, &d), [StoryRef::Part(f)]);
        assert_eq!(changed_stories(&d, &a), [StoryRef::Part(f)]);
    }
}
