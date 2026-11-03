//! Other people in a co-edited presentation, as PowerPoint for the web shows them: an outline in
//! their colour round the shapes they selected on the slide shown, and their name on a flag above
//! it while they edit.

use deckcraft_engine::DocState;
use deckcraft_engine::collab::SlidePlace;
use deckcraft_model::ShapeId;

use super::{Color32, Point, Pos2, Rect, Stroke, Xf};

/// Someone else in the presentation, where they are and whether they just edited.
#[derive(Clone, Debug, PartialEq)]
pub struct RemotePeer {
    pub name: String,
    /// Index into the suite's twelve people colours (the relay hands them out in join order).
    pub color: u8,
    pub place: Option<SlidePlace>,
    pub typing: bool,
}

/// The suite's people colours, the same in every app.
const PEER_COLORS: [[u8; 3]; 12] = [
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

pub fn peer_color(color: u8) -> Color32 {
    let [r, g, b] = PEER_COLORS[color as usize % 12];
    Color32::from_rgb(r, g, b)
}

/// What to draw for one person on the slide shown: their colour, the shapes they selected (or
/// are typing in), and the name for their flag when it shows.
#[derive(Debug, PartialEq)]
pub(crate) struct Mark {
    pub color: u8,
    pub shapes: Vec<ShapeId>,
    pub flag: Option<String>,
}

pub(crate) fn marks(st: &DocState, peers: &[RemotePeer]) -> Vec<Mark> {
    let Some(shown) = st.doc.slides.get(st.selection.slide).map(|s| s.id) else { return Vec::new() };
    peers
        .iter()
        .filter_map(|p| {
            let place = p.place.as_ref().filter(|pl| pl.slide == shown)?;
            let mut shapes = place.selection.shapes.clone();
            if let Some(t) = place.selection.text.as_ref().filter(|t| !t.notes && !shapes.contains(&t.shape)) {
                shapes.push(t.shape);
            }
            Some(Mark { color: p.color, shapes, flag: p.typing.then(|| p.name.clone()) })
        })
        .collect()
}

pub(super) fn paint_peers(st: &DocState, painter: &egui::Painter, xf: Xf, peers: &[RemotePeer]) {
    for m in marks(st, peers) {
        let c = peer_color(m.color);
        let mut top: Option<Pos2> = None;
        for id in &m.shapes {
            let Some(sh) = st.shape(*id) else { continue };
            let x = deckcraft_engine::cmd::xfrm_of(&st.doc, &st.selection, sh);
            let a = x.affine();
            let corners = [Point::new(0.0, 0.0), Point::new(x.w, 0.0), Point::new(x.w, x.h), Point::new(0.0, x.h)].map(|q| xf.to_screen(a * q));
            for i in 0..4 {
                painter.line_segment([corners[i], corners[(i + 1) % 4]], Stroke::new(2.0, c));
            }
            let first = corners.iter().copied().fold(corners[0], |m, q| if q.y < m.y || (q.y == m.y && q.x < m.x) { q } else { m });
            top.get_or_insert(first);
        }
        if let (Some(name), Some(at)) = (m.flag, top) {
            let galley = painter.layout_no_wrap(name, egui::FontId::proportional(11.0), Color32::WHITE);
            let pad = egui::vec2(4.0, 1.0);
            let size = galley.size() + pad * 2.0;
            let origin = Pos2::new(at.x, at.y - size.y);
            painter.rect_filled(Rect::from_min_size(origin, size), 2.0, c);
            painter.galley(origin + pad, galley, Color32::WHITE);
        }
    }
}

#[cfg(test)]
mod tests {
    use deckcraft_engine::Session;

    use super::*;

    #[test]
    fn people_on_the_slide_shown_are_marked_by_the_shapes_they_selected() {
        let mut s = Session::with_new();
        s.execute("slide.new", &serde_json::json!({})).unwrap();
        let st = s.active().unwrap();
        let (first, second) = (st.doc.slides[0].id, st.doc.slides[1].id);
        let title = st.doc.slides[1].shapes[0].id;
        let mut here = SlidePlace { slide: second, selection: Default::default() };
        here.selection.shapes = vec![title];
        let elsewhere = SlidePlace { slide: first, selection: Default::default() };
        let peers = [
            RemotePeer { name: "Ann".into(), color: 1, place: Some(here), typing: true },
            RemotePeer { name: "Bob".into(), color: 2, place: Some(elsewhere), typing: true },
        ];
        let m = marks(st, &peers);
        assert_eq!(m, vec![Mark { color: 1, shapes: vec![title], flag: Some("Ann".into()) }], "Bob is on another slide");
    }
}
