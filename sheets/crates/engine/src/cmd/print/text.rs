use super::draw::{Block, Look, rgb_of};
use super::{CellRef, Color, Font, HAlign, MAX_COLS, Page, Point, RangeRef, Sheet, Style, StyleTable, Underline, VAlign, Value};

const PAD_PX: f32 = 2.0;

/// Word-wraps `text` to `width` points.
pub(super) fn wrap_lines(text: &str, font: Font, size: f32, width: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let cand = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if line.is_empty() || gridcraft_pdf::text_width(font, size, &cand) <= width {
                line = cand;
            } else {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            }
            // Break a single over-long word by characters. The cut that fits is found by binary
            // search (each try measures one candidate prefix), so an over-long word costs
            // O(n log n) measurements instead of re-measuring the whole cut per character for
            // every emitted line (`a` × 100k pasted text would be cubic worst case).
            while gridcraft_pdf::text_width(font, size, &line) > width && line.chars().count() > 1 {
                let chars: Vec<char> = line.chars().collect();
                let (mut lo, mut hi) = (1usize, chars.len());
                while lo < hi {
                    let mid = lo + (hi - lo).div_ceil(2);
                    let cut: String = chars[..mid].iter().collect();
                    if gridcraft_pdf::text_width(font, size, &cut) <= width {
                        lo = mid;
                    } else {
                        hi = mid - 1;
                    }
                }
                if lo == chars.len() {
                    break;
                }
                out.push(chars[..lo].iter().collect());
                line = chars[lo..].iter().collect();
            }
            if out.len() > 2000 {
                return out;
            }
        }
        out.push(line);
    }
    out
}

fn font_for(style: &Style) -> Font {
    Font::styled(Font::family_for(&style.font.name), style.font.bold, style.font.italic)
}

fn text_align(align: HAlign, value: &Value) -> HAlign {
    match align {
        HAlign::General => match value {
            Value::Number(_) => HAlign::Right,
            Value::Bool(_) | Value::Error(_) => HAlign::Center,
            _ => HAlign::Left,
        },
        HAlign::CenterAcross | HAlign::Distributed => HAlign::Center,
        HAlign::Justify | HAlign::Fill => HAlign::Left,
        other => other,
    }
}

/// For each line [`wrap_lines`] makes of `text`, whether it ends its paragraph (justified text
/// leaves those as they are, but for Distributed); nothing for text that isn't `justified`.
fn paragraph_ends(justified: bool, text: &str, font: Font, size: f32, width: f32) -> Vec<bool> {
    if !justified {
        return Vec::new();
    }
    text.split('\n')
        .flat_map(|para| {
            let n = wrap_lines(para, font, size, width).len();
            (0..n).map(move |k| k + 1 == n)
        })
        .collect()
}

/// A cell's wrapped lines: which end their paragraph, the first's baseline and the line height.
struct Lines<'a> {
    lines: &'a [String],
    ends: &'a [bool],
    first_baseline: f32,
    line_h: f32,
}

/// Draws a justified cell's lines in the box at `x` (`avail` wide) of the cell from `y`, `h`
/// high: each line stretched but those that end a paragraph (all, `stretch_all`).
#[allow(clippy::too_many_arguments)]
fn justified_lines(
    pg: &mut gridcraft_pdf::Page,
    l: &Lines<'_>,
    (x, y, h, avail): (f32, f32, f32, f32),
    size: f32,
    font: Font,
    color: [u8; 3],
    stretch_all: bool,
    rtl_sheet: bool,
) {
    for (i, line) in l.lines.iter().enumerate() {
        let ly = l.first_baseline + i as f32 * l.line_h;
        let shown = !line.trim().is_empty() && ly - size <= y + h && ly >= y - l.line_h;
        let stretch = stretch_all || !l.ends.get(i).copied().unwrap_or(true);
        if shown {
            justified_line(pg, line.trim_end(), (x, ly, avail), size, font, color, stretch, rtl_sheet);
        }
    }
}

/// Draws `line` justified in the box from `x`, `avail` wide, on baseline `y`: `stretch`ed to the
/// width (Arabic with kashida, other text by its word gaps), else at its reading start. On a
/// right-to-left sheet the page is drawn mirrored, so "from the right" is from the left here.
#[allow(clippy::too_many_arguments)]
fn justified_line(
    pg: &mut gridcraft_pdf::Page,
    line: &str,
    (x, y, avail): (f32, f32, f32),
    size: f32,
    font: Font,
    color: [u8; 3],
    stretch: bool,
    rtl_sheet: bool,
) {
    use crate::justify::{TATWEEL, takes_kashida, with_kashida, word_positions};
    let width = |t: &str| gridcraft_pdf::text_width(font, size, t);
    let from_right = crate::display::reads_rtl(line) != rtl_sheet;
    let place = |at: f32, w: f32| if from_right { x + avail - at - w } else { x + at };
    if !stretch {
        pg.text(place(0.0, width(line)), y, size, font, color, line);
        return;
    }
    let tatweel = width(&TATWEEL.to_string());
    if tatweel > 0.0 && takes_kashida(line) {
        let n = (((avail - width(line)) / tatweel).floor().max(0.0) as usize).min(400);
        let s = with_kashida(line, n);
        pg.text(place(0.0, width(&s)), y, size, font, color, &s);
        return;
    }
    let words: Vec<&str> = line.split(' ').filter(|w| !w.is_empty()).collect();
    let widths: Vec<f32> = words.iter().map(|w| width(w)).collect();
    for ((word, w), at) in words.iter().zip(&widths).zip(word_positions(&widths, avail)) {
        pg.text(place(at, *w), y, size, font, color, word);
    }
}

/// Is a cell free for text to overflow into?
fn empty_for_overflow(sh: &Sheet, c: CellRef, merges: &[RangeRef]) -> bool {
    sh.value_ref(c).is_none_or(|v| v.is_empty()) && !merges.iter().any(|m| m.contains(c))
}

fn extend_text_bounds(sh: &Sheet, c: CellRef, merges: &[RangeRef], k: f32, need: f32, align: HAlign, bounds: (f32, f32)) -> (f32, f32) {
    let (mut clip_l, mut clip_r) = bounds;
    let grow = |dir: i64, mut remaining: f32| -> f32 {
        let mut ext = 0.0;
        let mut col = c.col as i64 + dir;
        while remaining > 0.0 && col >= 0 && col < MAX_COLS as i64 {
            let nc = CellRef::new(c.row, col as u32);
            if !empty_for_overflow(sh, nc, merges) {
                break;
            }
            let width = sh.col_width(nc.col) * k;
            ext += width;
            remaining -= width;
            col += dir;
            if ext > 20_000.0 {
                break;
            }
        }
        ext
    };
    match align {
        HAlign::Right => clip_l -= grow(-1, need),
        HAlign::Center => {
            clip_l -= grow(-1, need / 2.0);
            clip_r += grow(1, need / 2.0);
        }
        _ => clip_r += grow(1, need),
    }
    (clip_l, clip_r)
}

/// Unscaled width of the empty Center Across Selection cells right of `c`, which its text is centred over too.
pub(super) fn center_across_width(styles: &StyleTable, sh: &Sheet, c: CellRef, merges: &[RangeRef]) -> f32 {
    let mut ext = 0.0;
    let mut col = c.col + 1;
    while col < MAX_COLS && ext < 20_000.0 {
        let nc = CellRef::new(c.row, col);
        if !empty_for_overflow(sh, nc, merges) || styles.get(sh.style_id(nc)).align.h != HAlign::CenterAcross {
            break;
        }
        ext += sh.col_width(col);
        col += 1;
    }
    ext
}

pub(super) struct TextPlacement<'a> {
    pub cell: CellRef,
    pub look: &'a Look,
    pub rect: (f32, f32, f32, f32),
    pub merged: bool,
    pub merges: &'a [RangeRef],
    pub limits: (f32, f32),
}

pub(super) fn draw_text(pg: &mut Page, b: &Block, placement: TextPlacement<'_>) {
    let TextPlacement { cell: c, look: l, rect, merged, merges, limits } = placement;
    let (x, y, w, h) = rect;
    let sh = b.sh;
    let wb = b.wb;
    let v = sh.value(c);
    if v.is_empty() || l.cf.as_ref().is_some_and(|cf| cf.hide_value) {
        return;
    }
    if !sh.show_zeros && v.as_f64() == Some(0.0) {
        return;
    }
    let st = &l.style;
    let mut size = st.font.size.clamp(1.0, 409.0) * b.scale;
    let font = font_for(st);
    let pad = PAD_PX * b.k;
    let indent = st.align.indent as f32 * 9.0 * b.k;
    let avail = (w - 2.0 * pad - indent).max(0.0);
    let code = st.num_fmt.as_str();
    let formatted = crate::display::format(&v, code, wb);
    let mut color = formatted.color.and_then(gridcraft_numfmt::FormatColor::rgb).or_else(|| rgb_of(st.font.color, wb)).unwrap_or([0, 0, 0]);
    if l.cf.as_ref().and_then(|x| x.style.as_ref()).is_some_and(|s| s.font.color != Color::Auto) {
        color = rgb_of(st.font.color, wb).unwrap_or(color);
    }
    let is_num = matches!(v, Value::Number(_));
    // Justify and Distributed wrap, as on screen.
    let justify = matches!(st.align.h, HAlign::Justify | HAlign::Distributed);
    let wrap = st.align.wrap || justify;
    let mut text = if sh.show_formulas {
        crate::display::cell_text(wb, sh, c)
    } else if code == "General"
        && let Value::Number(n) = v
    {
        let digit = gridcraft_pdf::text_width(font, size, "0").max(0.1);
        let max_chars = (avail / digit).floor().max(1.0) as usize;
        gridcraft_numfmt::format_general_fit(n, max_chars.min(11)).unwrap_or_else(|| "#".repeat(max_chars.min(255)))
    } else {
        // The look's code, which a conditional format can override (#1163).
        crate::display::value_text_as(wb, &v, code)
    };
    if !wrap {
        text = text.replace(['\n', '\r'], " ");
    }
    if text.is_empty() {
        return;
    }
    let mut tw = gridcraft_pdf::text_width(font, size, &text);
    if st.align.shrink && !wrap && tw > avail && tw > 0.0 {
        size = (size * avail / tw).max(1.0);
        tw = gridcraft_pdf::text_width(font, size, &text);
    }
    if is_num && !wrap && tw > avail + 0.01 && !sh.show_formulas {
        let hash = gridcraft_pdf::text_width(font, size, "#").max(0.1);
        text = "#".repeat(((avail / hash).floor().max(1.0) as usize).min(255));
        tw = gridcraft_pdf::text_width(font, size, &text);
    }
    let h_align = match text_align(st.align.h, &v) {
        // General text that reads right to left (Arabic, Hebrew) starts at the right, as on screen.
        HAlign::Left if st.align.h == HAlign::General && !sh.right_to_left && crate::display::reads_rtl(&text) => HAlign::Right,
        a => a,
    };
    let rot = st.align.rotation;
    let rotated = rot != 0 && rot != 255;
    let ends = paragraph_ends(justify, &text, font, size, avail.max(size));
    // Rotated text wraps along its own direction: upright (±90°) text to the row height.
    let lines: Vec<String> = match (wrap, rotated) {
        (true, false) => wrap_lines(&text, font, size, avail.max(size)),
        (true, true) if rot.abs() == 90 => wrap_lines(&text, font, size, (h - 2.0 * pad).max(size)),
        (true, true) => vec![text.lines().next().unwrap_or_default().to_string()],
        (false, _) => vec![text],
    };
    let line_h = size * 1.2;
    let n = lines.len().max(1) as f32;
    let first_baseline = match st.align.v {
        VAlign::Top => y + pad + size * 0.8,
        VAlign::Center | VAlign::Justify | VAlign::Distributed => y + (h - n * line_h) / 2.0 + line_h * 0.5 + size * 0.33,
        VAlign::Bottom => y + h - pad - size * 0.22 - (n - 1.0) * line_h,
    };
    // Overflow into empty neighbours (single-line text, not merged).
    // Center Across Selection spans the empty Center Across cells to its right; the text stays within that run.
    let spans = st.align.h == HAlign::CenterAcross && !wrap && !merged && rot == 0;
    let across = if spans { center_across_width(&wb.styles, sh, c, merges) * b.k } else { 0.0 };
    let (mut clip_l, mut clip_r) = (x, x + w + across);
    if !wrap && !merged && !is_num && rot.abs() != 90 && across <= 0.0 && tw + 2.0 * pad + indent > w {
        let need = tw + 2.0 * pad + indent - w;
        (clip_l, clip_r) = extend_text_bounds(sh, c, merges, b.k, need, h_align, (clip_l, clip_r));
    }
    let clip_l = clip_l.max(limits.0);
    let clip_r = clip_r.min(limits.1);
    pg.save();
    pg.clip_rect(clip_l, y, (clip_r - clip_l).max(0.0), h);
    if rotated {
        // Rotated text: anchored at the bottom-left (upward) or top-left (downward) of the cell.
        // Wrapped lines stack rightward for upward text and leftward for downward text, so
        // downward text starts its first line far enough in for the last to end at the left pad.
        let deg = (rot as f32).clamp(-90.0, 90.0);
        let angle = -deg.to_radians();
        let last = lines.len().saturating_sub(1) as f32;
        for (i, t) in lines.iter().enumerate() {
            let i = i as f32;
            let (ox, oy) = if deg > 0.0 {
                (x + pad + size * 0.8 * deg.to_radians().sin() + i * line_h, y + h - pad)
            } else {
                (x + pad + (last - i) * line_h, y + pad + size * 0.8)
            };
            pg.text_rotated(Point::new(ox, oy), size, font, color, t, angle);
        }
        pg.restore();
        return;
    }
    if rot == 255 {
        // Vertical stacked text.
        let chars: Vec<char> = lines.concat().chars().take(500).collect();
        let mut yy = y + pad + size * 0.8;
        for ch in chars {
            let s = ch.to_string();
            let cw = gridcraft_pdf::text_width(font, size, &s);
            pg.text(x + (w - cw) / 2.0, yy, size, font, color, &s);
            yy += line_h;
            if yy > y + h + line_h {
                break;
            }
        }
        pg.restore();
        return;
    }
    if justify {
        let lines = Lines { lines: &lines, ends: &ends, first_baseline, line_h };
        let stretch_all = st.align.h == HAlign::Distributed;
        justified_lines(pg, &lines, (x + pad + indent, y, h, avail), size, font, color, stretch_all, sh.right_to_left);
        pg.restore();
        return;
    }
    for (i, line) in lines.iter().enumerate() {
        let lw = gridcraft_pdf::text_width(font, size, line);
        let lx = match h_align {
            HAlign::Right => x + w - pad - lw - indent,
            HAlign::Center => x + (w + across - lw) / 2.0,
            _ => x + pad + indent,
        };
        let ly = first_baseline + i as f32 * line_h;
        if ly - size > y + h || ly < y - line_h {
            continue;
        }
        pg.text(lx, ly, size, font, color, line);
        if st.font.underline != Underline::None {
            let uw = (size * 0.06).max(0.2);
            pg.stroke_line(lx, ly + size * 0.12, lx + lw, ly + size * 0.12, uw, color);
            if matches!(st.font.underline, Underline::Double | Underline::DoubleAccounting) {
                pg.stroke_line(lx, ly + size * 0.24, lx + lw, ly + size * 0.24, uw, color);
            }
        }
        if st.font.strike {
            pg.stroke_line(lx, ly - size * 0.3, lx + lw, ly - size * 0.3, (size * 0.06).max(0.2), color);
        }
    }
    pg.restore();
}
