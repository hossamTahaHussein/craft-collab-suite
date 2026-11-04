//! Arabic and Hebrew vowels in the text readers copy (as Craft Slides writes them). A vowel mark
//! shaped as a glyph of its own sits over or under its letter; a reader placed it by where it is
//! drawn (before its letter, in a right-to-left word: الِكَتاُب for الكِتَابُ), and Chrome's PDFium
//! took a raised mark for a break in the word. So, as Craft Docs does, each vowel's character is put
//! on the baseline just past its letter's trailing (left) edge, where a reader puts the next
//! character: the words copy and search whole, vowels in place.

use std::ops::Range;

/// A vowel or other combining mark of Arabic or Hebrew (harakat, tanween, shadda, sukun, points).
pub(crate) fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0591..=0x05BD | 0x05BF | 0x05C1..=0x05C2 | 0x05C4..=0x05C5 | 0x05C7 | 0x0610..=0x061A | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7..=0x06E8 | 0x06EA..=0x06ED | 0x08D3..=0x08E1 | 0x08E3..=0x08FF)
}

/// One glyph of the text layer: id, x and y (as laid out) and the text it stands for.
pub(crate) type Item = (u32, f64, f64, Range<usize>);

/// The text layer's glyphs for a run (`glyphs` in display order, each with its text `ranges` in
/// `text`; `advance` gives a glyph's advance), or `None` when the run has no vowels (it is
/// written as it is). The result is in display order, each glyph with text of its own: a letter
/// its letters, a vowel glyph its vowel, at the letter's trailing edge on the baseline. A vowel
/// drawn as part of its letter's glyph (Hebrew holam male) is carried by an invisible copy of
/// that glyph there.
pub(crate) fn text_items(
    text: &str,
    glyphs: &[(u32, f64, f64)],
    ranges: &[Range<usize>],
    size: f64,
    advance: impl Fn(u32) -> f64,
) -> Option<Vec<Item>> {
    let n = glyphs.len().min(ranges.len());
    let marks_in = |r: &Range<usize>| text.get(r.clone()).map_or(0, |t| t.len() - t.trim_end_matches(is_mark).len());
    // A vowel glyph: no advance, in a cluster whose text ends in vowels.
    let is_vowel = |i: usize| advance(glyphs[i].0) <= 0.0 && marks_in(&ranges[i]) > 0;
    if !(0..n).any(|i| marks_in(&ranges[i]) > 0) {
        return None;
    }
    // The baseline: a letter's y (marks sit higher or lower).
    let base_y = (0..n).find(|&i| !is_vowel(i)).map_or(glyphs.first()?.2, |i| glyphs[i].2);
    let mut out: Vec<Item> = Vec::with_capacity(n);
    let mut done = vec![false; n];
    for i in 0..n {
        if done[i] {
            continue;
        }
        let r = ranges[i].clone();
        let cluster: Vec<usize> = (i..n).filter(|&j| ranges[j] == r).collect();
        let (vowels, letters): (Vec<usize>, Vec<usize>) = cluster.iter().partition(|&&j| is_vowel(j));
        for &j in &cluster {
            done[j] = true;
        }
        let marks = marks_in(&r);
        let split = r.end - marks;
        if marks == 0 || letters.is_empty() || split == r.start {
            out.extend(cluster.iter().map(|&j| (glyphs[j].0, glyphs[j].1, glyphs[j].2, r.clone())));
            continue;
        }
        // No glyph of their own: the vowels go on a copy of the letter's.
        let vowels = if vowels.is_empty() { vec![letters[0]] } else { vowels };
        // The letter keeps the letters (a whole vowel cluster stays on its own glyph).
        let lead = letters.first().copied();
        for &j in &letters {
            let own = if Some(j) == lead && split > r.start { r.start..split } else { r.start..r.start };
            out.push((glyphs[j].0, glyphs[j].1, glyphs[j].2, own));
        }
        // The letter's trailing edge: its left one in a right-to-left run (left of every letter
        // glyph of the cluster).
        let edge = letters.iter().map(|&j| glyphs[j].1).fold(f64::INFINITY, f64::min);
        let edge = if edge.is_finite() { edge } else { glyphs[i].1 };
        // Each vowel on a vowel glyph, in reading order, a little further past the letter.
        let chars: Vec<(usize, char)> = text[split..r.end].char_indices().map(|(o, c)| (split + o, c)).collect();
        // One character each (a reader reverses a glyph's several characters in a right-to-left
        // run): more vowels than glyphs go on copies of the last.
        for (k, &(at, c)) in chars.iter().enumerate() {
            let Some(&g) = vowels.get(k).or(vowels.last()) else { break };
            out.push((glyphs[g].0, edge - 0.001 * size * (k + 1) as f64, base_y, at..at + c.len_utf8()));
        }
    }
    out.sort_by(|a, b| a.1.total_cmp(&b.1));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::text_items;

    /// The text a reader gets from `items`, reading a right-to-left run from its right end.
    fn read(text: &str, items: &[super::Item]) -> String {
        items.iter().rev().map(|i| &text[i.3.clone()]).collect()
    }

    /// Glyphs 100 and up are vowels (no advance).
    fn advance(g: u32) -> f64 {
        if g >= 100 { 0.0 } else { 500.0 }
    }

    /// As the shaper gives them: right to left, each vowel glyph first in its letter's cluster
    /// and drawn raised, right of the letter's origin.
    fn shaped() -> (&'static str, Vec<(u32, f64, f64)>, Vec<std::ops::Range<usize>>) {
        // ا ل كِ تَ: alef 0..2, lam 2..4, kaf+kasra 4..8, teh+fatha 8..12.
        let text = "الكِتَ";
        let glyphs = vec![(101, 4.0, 44.0), (3, 0.0, 50.0), (102, 10.0, 56.0), (4, 7.0, 50.0), (5, 14.0, 50.0), (6, 18.0, 50.0)];
        let ranges = vec![8..12, 8..12, 4..8, 4..8, 2..4, 0..2];
        (text, glyphs, ranges)
    }

    #[test]
    fn each_vowel_follows_its_letter_on_the_baseline_with_text_of_its_own() {
        let (text, glyphs, ranges) = shaped();
        let items = text_items(text, &glyphs, &ranges, 20.0, advance).unwrap();
        assert_eq!(read(text, &items), text);
        assert!(items.iter().all(|i| i.2 == 50.0), "every glyph of the text on the baseline: {items:?}");
        let mut seen: Vec<_> = items.iter().map(|i| i.3.clone()).collect();
        seen.dedup();
        assert_eq!(seen.len(), items.len(), "no two glyphs share a text: {items:?}");
        // Display order is reading order backwards: the ranges only go down.
        assert!(items.windows(2).all(|w| w[0].3.start >= w[1].3.end), "{items:?}");
    }

    #[test]
    fn two_vowels_on_one_letter_keep_their_order() {
        // بَّ as beh, shadda, fatha in one cluster.
        let text = "بّ\u{064E}";
        let glyphs = [(1, 0.0, 50.0), (101, 2.0, 40.0), (102, 2.0, 38.0)];
        let ranges = [0..6, 0..6, 0..6];
        let items = text_items(text, &glyphs, &ranges, 20.0, advance).unwrap();
        assert_eq!(read(text, &items), text);
        // One glyph for both vowels: it is written twice, a vowel each.
        let one = text_items(text, &glyphs[..2], &ranges[..2], 20.0, advance).unwrap();
        assert_eq!(read(text, &one), text);
        assert!(one.iter().all(|i| text[i.3.clone()].chars().count() == 1), "{one:?}");
    }

    #[test]
    fn runs_without_vowel_glyphs_are_left_as_they_are() {
        assert!(text_items("مرحبا", &[(1, 0.0, 0.0), (2, 5.0, 0.0)], &[2..4, 0..2], 12.0, advance).is_none());
        assert!(text_items("Hello", &[(1, 0.0, 0.0)], std::slice::from_ref(&(0..5)), 12.0, advance).is_none());
    }

    #[test]
    fn a_vowel_inside_its_letters_glyph_is_carried_by_a_copy_of_it() {
        // שלוֹם with holam male one glyph: mem, vav+holam, lamed, shin, right to left.
        let text = "שלוֹם";
        let glyphs = [(1, 0.0, 50.0), (2, 10.0, 50.0), (3, 14.0, 50.0), (4, 20.0, 50.0)];
        let ranges = [8..10, 4..8, 2..4, 0..2];
        let items = text_items(text, &glyphs, &ranges, 20.0, advance).unwrap();
        assert_eq!(read(text, &items), text);
        assert_eq!(items.iter().filter(|i| i.0 == 2).count(), 2, "vav drawn twice in the text: {items:?}");
    }
}
