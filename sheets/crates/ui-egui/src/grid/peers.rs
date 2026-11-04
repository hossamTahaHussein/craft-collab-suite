//! Other people in a co-edited workbook, as Google Sheets and Excel for the web show them: a
//! border in their colour round their active cell, a light fill over what they selected, and
//! their name on a flag above the cell while they edit (and always for the one followed).

use egui::{Color32, Painter, Pos2, Rect, Stroke, StrokeKind, vec2};
use gridcraft_engine::core::RangeRef;
use gridcraft_engine::model::Sheet;

use super::geo::Geo;

/// Someone else in the workbook, where they are and whether they just edited.
#[derive(Clone, Debug, PartialEq)]
pub struct RemotePeer {
    pub name: String,
    /// Index into [`PEER_COLORS`] (the relay hands colours out in join order).
    pub color: u8,
    pub place: Option<gridcraft_engine::collab::SheetPlace>,
    pub typing: bool,
}

/// The suite's people colours, the same in every app (light and dark themes).
pub const PEER_COLORS: [[u8; 3]; 12] = [
    [0xD9, 0x48, 0x2B],
    [0x1E, 0x7B, 0xD6],
    [0x2E, 0x9E, 0x4F],
    [0x8E, 0x44, 0xC9],
    [0xD6, 0x8A, 0x00],
    [0x00, 0x96, 0x96],
    [0xC9, 0x2E, 0x8A],
    [0x5A, 0x6B, 0x00],
    [0x34, 0x4C, 0xB7],
    [0xB8, 0x56, 0x2A],
    [0x00, 0x7F, 0x5F],
    [0x7A, 0x5C, 0xFF],
];
pub const PEER_COLORS_DARK: [[u8; 3]; 12] = [
    [0xFF, 0x7A, 0x5C],
    [0x5C, 0xAE, 0xFF],
    [0x5F, 0xD0, 0x7E],
    [0xC3, 0x8B, 0xFF],
    [0xFF, 0xC1, 0x4D],
    [0x3F, 0xD5, 0xD5],
    [0xFF, 0x6F, 0xC1],
    [0xB5, 0xC8, 0x3A],
    [0x8C, 0xA0, 0xFF],
    [0xFF, 0x9E, 0x6E],
    [0x4F, 0xD1, 0xA8],
    [0xB4, 0xA2, 0xFF],
];

/// The colour of `author`'s suggestions: one of the people colours, the same for a name wherever
/// it shows.
pub fn suggestion_color(author: &str, dark: bool) -> Color32 {
    let h = author.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    peer_color((h % 8) as u8, dark)
}

pub fn peer_color(color: u8, dark: bool) -> Color32 {
    let [r, g, b] = if dark { PEER_COLORS_DARK } else { PEER_COLORS }[color as usize % 12];
    Color32::from_rgb(r, g, b)
}

/// What to draw for one person on sheet `sheet`: their colour, their active cell (as its merged
/// area), the areas they selected, and the name for their flag when it shows.
#[derive(Debug, PartialEq)]
pub(crate) struct Mark {
    pub color: u8,
    pub active: RangeRef,
    pub ranges: Vec<RangeRef>,
    pub flag: Option<String>,
}

pub(crate) fn marks(sh: &Sheet, peers: &[RemotePeer]) -> Vec<Mark> {
    peers
        .iter()
        .filter_map(|p| {
            let place = p.place.as_ref().filter(|pl| pl.sheet == sh.name)?;
            let sel = &place.selection;
            let active = sh.merge_at(sel.active).unwrap_or_else(|| RangeRef::cell(sel.active));
            let ranges = sel.ranges.iter().copied().filter(|r| *r != active).collect();
            Some(Mark { color: p.color, active, ranges, flag: p.typing.then(|| p.name.clone()) })
        })
        .collect()
}

fn range_rect(geo: &Geo, sh: &Sheet, r: RangeRef) -> Rect {
    geo.cell_rect(sh, r.start).union(geo.cell_rect(sh, r.end))
}

/// Paint everyone on this sheet into `p` (the pane's painter, clipped to it).
pub(crate) fn paint_peers(p: &Painter, geo: &Geo, sh: &Sheet, peers: &[RemotePeer]) {
    let dark = p.ctx().global_style().visuals.dark_mode;
    for m in marks(sh, peers) {
        let c = peer_color(m.color, dark);
        for r in &m.ranges {
            p.rect_filled(range_rect(geo, sh, *r), 0.0, c.gamma_multiply(0.12));
        }
        let cell = range_rect(geo, sh, m.active);
        p.rect_stroke(cell, 0.0, Stroke::new(2.0, c), StrokeKind::Inside);
        if let Some(name) = m.flag {
            let font = egui::FontId::proportional(11.0);
            let galley = p.layout_no_wrap(name, font, Color32::WHITE);
            let pad = vec2(4.0, 1.0);
            let size = galley.size() + pad * 2.0;
            let at = Pos2::new(cell.left(), cell.top() - size.y);
            p.rect_filled(Rect::from_min_size(at, size), 2.0, c);
            p.galley(at + pad, galley, Color32::WHITE);
        }
    }
}

/// The rows each person's active cell covers on sheet `sheet`, with their colour: `(first row,
/// last row, colour)`, for the ticks on the row header.
pub(crate) fn row_ticks(sh: &Sheet, peers: &[RemotePeer]) -> Vec<(u32, u32, u8)> {
    marks(sh, peers).into_iter().map(|m| (m.active.start.row, m.active.end.row, m.color)).collect()
}

/// A tick in each person's colour on the row header beside the rows they are on, at the header's
/// edge against the cells.
pub(crate) fn paint_row_ticks(p: &Painter, geo: &Geo, sh: &Sheet, peers: &[RemotePeer]) {
    let dark = p.ctx().global_style().visuals.dark_mode;
    let edge = geo.cells.left();
    let clip = p.with_clip_rect(Rect::from_min_max(egui::pos2(geo.head_left(), geo.cells.top()), egui::pos2(edge, geo.rect.bottom())));
    for (first, last, color) in row_ticks(sh, peers) {
        let (top, bottom) = (geo.y(sh, first), geo.y(sh, last) + sh.row_height(last) * geo.z);
        if bottom <= top {
            continue;
        }
        clip.rect_filled(Rect::from_min_max(egui::pos2(edge - 4.0, top), egui::pos2(edge - 1.0, bottom)), 0.0, peer_color(color, dark));
    }
}

/// The first letter of the first and last words of a name ("Ann Lee" → "AL").
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let first = |w: &&str| w.chars().next();
    let mut out: String = words.first().and_then(first).into_iter().collect();
    if words.len() > 1 {
        out.extend(words.last().and_then(first));
    }
    out.to_uppercase()
}

/// The title bar's people: up to five avatars (in a right-to-left layout, the first one
/// rightmost) and how many more there are; returns the person clicked, to go to them.
pub fn avatars(ui: &mut egui::Ui, peers: &[RemotePeer]) -> Option<usize> {
    const SHOWN: usize = 5;
    let dark = ui.global_style().visuals.dark_mode;
    let mut clicked = None;
    if peers.len() > SHOWN {
        ui.label(egui::RichText::new(format!("+{}", peers.len() - SHOWN)).small());
    }
    for (i, p) in peers.iter().enumerate().take(SHOWN) {
        let color = p.color;
        let fill = peer_color(color, dark);
        let (r, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
        ui.painter().circle_filled(r.center(), 11.0, fill);
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, initials(&p.name), egui::FontId::proportional(10.0), Color32::WHITE);
        let name = p.name.clone();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
        if resp.on_hover_text(format!("Go to {}", p.name)).clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

#[cfg(test)]
mod tests {
    use gridcraft_engine::Selection;
    use gridcraft_engine::collab::SheetPlace;
    use gridcraft_engine::core::CellRef;

    use super::*;

    fn peer(sheet: &str, a1: &str, typing: bool) -> RemotePeer {
        let place = SheetPlace { sheet: sheet.into(), selection: Selection::at(CellRef::parse(a1).unwrap()) };
        RemotePeer { name: "Ann".into(), color: 3, place: Some(place), typing }
    }

    #[test]
    fn people_on_this_sheet_are_marked_and_their_name_shows_while_they_edit() {
        let sh = Sheet::new("Sheet1");
        let m = marks(&sh, &[peer("Sheet1", "B2", true), peer("Sheet2", "C3", true), peer("Sheet1", "D4", false)]);
        assert_eq!(m.len(), 2, "the one on another sheet isn't here");
        assert_eq!(m[0].active, RangeRef::parse("B2").unwrap());
        assert_eq!(m[0].flag.as_deref(), Some("Ann"));
        assert_eq!(m[1].flag, None, "not editing: no flag");
    }

    #[test]
    fn their_selected_areas_are_filled_apart_from_the_active_cell() {
        let sh = Sheet::new("Sheet1");
        let mut p = peer("Sheet1", "A1", false);
        if let Some(pl) = p.place.as_mut() {
            pl.selection.ranges = vec![RangeRef::parse("A1:C3").unwrap()];
        }
        let m = marks(&sh, &[p]);
        assert_eq!(m[0].ranges, vec![RangeRef::parse("A1:C3").unwrap()]);
    }

    /// The row header shows a tick in each person's colour on the rows of their active cell (a
    /// merged one's rows too), as Docs' margin bar marks the lines people are on.
    #[test]
    fn row_headers_tick_the_rows_people_are_on() {
        let mut sh = Sheet::new("Sheet1");
        sh.merges.push(RangeRef::parse("D4:E6").unwrap());
        let ticks = row_ticks(&sh, &[peer("Sheet1", "B2", false), peer("Sheet2", "C9", false), peer("Sheet1", "D4", true)]);
        assert_eq!(ticks, vec![(1, 1, 3), (3, 5, 3)]);
    }

    #[test]
    fn initials_are_the_first_and_last_words() {
        assert_eq!(initials("Ann Lee"), "AL");
        assert_eq!(initials("bob"), "B");
        assert_eq!(initials("حسام طه"), "حط");
    }

    #[test]
    fn colours_come_round_after_twelve() {
        assert_eq!(peer_color(13, false), peer_color(1, false));
        assert_ne!(peer_color(1, false), peer_color(1, true));
    }
}
