//! Justified cell text (Justify and Distributed alignment): a line that doesn't end its paragraph
//! is stretched to the cell's width. Arabic stretches its words with kashida (tatweel, U+0640)
//! between joined letters, as Craft Docs and Craft Slides justify it; other text widens the gaps
//! between its words.

/// The tatweel: a joining letter's stroke drawn longer.
pub const TATWEEL: char = '\u{0640}';

/// An Arabic letter (no marks, digits or punctuation).
fn arabic_letter(c: char) -> bool {
    matches!(c as u32, 0x0620..=0x063F | 0x0641..=0x064A | 0x066E..=0x066F | 0x0671..=0x06D3 | 0x06D5 | 0x06EE..=0x06EF | 0x06FA..=0x06FC | 0x06FF)
}

/// A letter that joins only to the letter before it: nothing joins after it (alef, dal, reh, waw…).
fn joins_before_only(c: char) -> bool {
    matches!(
        c,
        'ا' | 'أ'
            | 'إ'
            | 'آ'
            | 'ٱ'
            | 'د'
            | 'ذ'
            | 'ر'
            | 'ز'
            | 'و'
            | 'ؤ'
            | 'ة'
            | 'ى'
            | 'ء'
            | 'ڈ'
            | 'ڑ'
            | 'ژ'
            | 'ۀ'
            | 'ے'
            | 'ۓ'
            | 'ە'
            | 'ۇ'
            | 'ۆ'
            | 'ۈ'
            | 'ۋ'
            | 'ۅ'
            | 'ۉ'
            | 'ې'
    )
}

/// A vowel or other mark drawn on the letter before it.
fn mark(c: char) -> bool {
    matches!(c as u32, 0x0610..=0x061A | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7..=0x06E8 | 0x06EA..=0x06ED)
}

/// Where kashida may go in `chars`: after the letter (and its marks) at each index returned, the
/// letter joining the next one (never inside lam-alef, which is one ligature). The last such point
/// of each word comes first, then the one before it, word by word (where a writer stretches
/// first).
pub fn kashida_points(chars: &[char]) -> Vec<usize> {
    let mut words: Vec<Vec<usize>> = vec![Vec::new()];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !arabic_letter(c) && c != TATWEEL && !mark(c) {
            if !words.last().is_some_and(Vec::is_empty) {
                words.push(Vec::new());
            }
            i += 1;
            continue;
        }
        // After the letter's marks.
        let mut end = i;
        while chars.get(end + 1).copied().is_some_and(mark) {
            end += 1;
        }
        let next = chars.get(end + 1).copied();
        let lam_alef = c == 'ل' && matches!(next, Some('ا' | 'أ' | 'إ' | 'آ'));
        if arabic_letter(c)
            && !joins_before_only(c)
            && !lam_alef
            && next.is_some_and(arabic_letter)
            && let Some(w) = words.last_mut()
        {
            w.push(end);
        }
        i = end + 1;
    }
    let mut out = Vec::new();
    let deepest = words.iter().map(Vec::len).max().unwrap_or(0);
    for round in 0..deepest {
        for w in &words {
            if let Some(&p) = w.len().checked_sub(round + 1).and_then(|k| w.get(k)) {
                out.push(p);
            }
        }
    }
    out
}

/// `text` with `count` tatweels spread over its kashida points (see [`kashida_points`]): round
/// after round, so each point gets one before any gets two. Unchanged without points.
pub fn with_kashida(text: &str, count: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let points = kashida_points(&chars);
    if points.is_empty() || count == 0 {
        return text.to_string();
    }
    let mut extra = vec![0usize; chars.len()];
    for k in 0..count {
        extra[points[k % points.len()]] += 1;
    }
    let mut out = String::with_capacity(text.len() + count * 2);
    for (i, c) in chars.iter().enumerate() {
        out.push(*c);
        out.extend(std::iter::repeat_n(TATWEEL, extra[i]));
    }
    out
}

/// Whether a justified line stretches with kashida: it has Arabic letters that can take it.
pub fn takes_kashida(text: &str) -> bool {
    !kashida_points(&text.chars().collect::<Vec<_>>()).is_empty()
}

/// The x of each word of a line justified to `width` (words' widths in `words`, in order): the
/// gaps share what's left. A single word stays where it starts.
pub fn word_positions(words: &[f32], width: f32) -> Vec<f32> {
    let total: f32 = words.iter().sum();
    let gaps = words.len().saturating_sub(1);
    let gap = if gaps == 0 { 0.0 } else { ((width - total) / gaps as f32).max(0.0) };
    let mut x = 0.0;
    words
        .iter()
        .map(|w| {
            let at = x;
            x += w + gap;
            at
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kashida_goes_between_joined_letters_last_first() {
        // كتاب: kaf joins teh, teh joins alef, alef joins nothing after it.
        let chars: Vec<char> = "كتاب".chars().collect();
        assert_eq!(kashida_points(&chars), vec![1, 0]);
        // Two words: each word's last point first.
        let chars: Vec<char> = "سلم كتب".chars().collect();
        assert_eq!(kashida_points(&chars), vec![1, 5, 0, 4]);
        // Never inside lam-alef, never after a letter that joins nothing, never in Latin.
        assert!(kashida_points(&"لا".chars().collect::<Vec<_>>()).is_empty());
        assert!(kashida_points(&"دار".chars().collect::<Vec<_>>()).is_empty());
        assert!(kashida_points(&"Hello".chars().collect::<Vec<_>>()).is_empty());
    }

    #[test]
    fn kashida_follows_the_vowels_of_its_letter() {
        // كِتَب: after kasra and after fatha, not between a letter and its vowel.
        let chars: Vec<char> = "كِتَب".chars().collect();
        assert_eq!(kashida_points(&chars), vec![3, 1]);
        assert_eq!(with_kashida("كِتَب", 2), "كِـتَـب");
    }

    #[test]
    fn tatweels_spread_before_doubling() {
        assert_eq!(with_kashida("كتاب", 1), "كتـاب");
        assert_eq!(with_kashida("كتاب", 3), "كـتــاب");
        assert_eq!(with_kashida("Hello", 3), "Hello");
        assert!(takes_kashida("مرحبا بالعالم") && !takes_kashida("Hello") && !takes_kashida("دار"));
    }

    #[test]
    fn gaps_share_the_space_left() {
        assert_eq!(word_positions(&[10.0, 20.0, 10.0], 60.0), vec![0.0, 20.0, 50.0]);
        assert_eq!(word_positions(&[10.0], 60.0), vec![0.0]);
        // Wider than the line: no negative gaps.
        assert_eq!(word_positions(&[40.0, 40.0], 60.0), vec![0.0, 40.0]);
    }
}
