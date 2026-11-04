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

/// A person's colour (an index into the palette) as CSS (`#rrggbb`), for the page that embeds
/// the editor.
pub fn peer_css_color(color: u8) -> String {
    let [r, g, b] = PEER_COLORS[color as usize % 12];
    format!("#{r:02x}{g:02x}{b:02x}")
}

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
    // Their text selections and carets, in their colours.
    for (color, ts) in text_carets(st, peers) {
        let c = peer_color(color);
        for quad in super::overlays::text_marks(st, &ts).iter().flat_map(|m| &m.selection) {
            painter.add(egui::Shape::convex_polygon(quad.iter().map(|q| xf.to_screen(*q)).collect(), c.gamma_multiply(0.25), Stroke::NONE));
        }
        if !ts.is_range()
            && let Some((top, bottom)) = deckcraft_engine::cmd::text::caret_line(st, &ts)
        {
            painter.line_segment([xf.to_screen(top), xf.to_screen(bottom)], Stroke::new(2.0, c));
        }
    }
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

/// The colours of the people on slide `slide`, for the dots on its thumbnail.
pub(crate) fn colors_on(peers: &[RemotePeer], slide: deckcraft_model::SlideId) -> Vec<u8> {
    peers.iter().filter(|p| p.place.as_ref().is_some_and(|pl| pl.slide == slide)).map(|p| p.color).collect()
}

/// Dots in the colours of the people on a slide, along the top of its thumbnail `img` from its
/// reading-end corner.
pub(crate) fn paint_thumb_dots(painter: &egui::Painter, img: Rect, colors: &[u8], rtl: bool) {
    const R: f32 = 4.0;
    for (i, c) in colors.iter().take(6).enumerate() {
        let dx = R + 3.0 + i as f32 * (2.0 * R + 2.0);
        let x = if rtl { img.left() + dx } else { img.right() - dx };
        painter.circle(Pos2::new(x, img.top() + R + 3.0), R, peer_color(*c), Stroke::new(1.0, Color32::WHITE));
    }
}

/// The text people are typing in on the slide shown (not the notes), with their colours.
pub(crate) fn text_carets(st: &DocState, peers: &[RemotePeer]) -> Vec<(u8, deckcraft_engine::TextSel)> {
    let Some(shown) = st.doc.slides.get(st.selection.slide).map(|s| s.id) else { return Vec::new() };
    peers
        .iter()
        .filter_map(|p| {
            let place = p.place.as_ref().filter(|pl| pl.slide == shown)?;
            let ts = place.selection.text.as_ref().filter(|t| !t.notes && st.shape(t.shape).is_some())?;
            Some((p.color, ts.clone()))
        })
        .collect()
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
    let mut clicked = None;
    if peers.len() > SHOWN {
        ui.label(egui::RichText::new(format!("+{}", peers.len() - SHOWN)).small());
    }
    for (i, p) in peers.iter().enumerate().take(SHOWN) {
        let color = p.color;
        let fill = peer_color(color);
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

    /// The thumbnails pane puts a dot in each person's colour on the slide they are on.
    #[test]
    fn each_slide_knows_who_is_on_it() {
        let mut s = Session::with_new();
        s.execute("slide.new", &serde_json::json!({})).unwrap();
        let st = s.active().unwrap();
        let (first, second) = (st.doc.slides[0].id, st.doc.slides[1].id);
        let on =
            |slide, color| RemotePeer { name: "P".into(), color, place: Some(SlidePlace { slide, selection: Default::default() }), typing: false };
        let peers = [on(second, 1), on(first, 4), on(second, 7), RemotePeer { name: "Nowhere".into(), color: 9, place: None, typing: false }];
        assert_eq!(colors_on(&peers, second), vec![1, 7]);
        assert_eq!(colors_on(&peers, first), vec![4]);
    }

    /// Someone typing in a text box on the slide shown has a caret there (in their colour); in
    /// the speaker notes or on another slide, not.
    #[test]
    fn carets_of_people_typing_on_the_slide_shown() {
        let mut s = Session::with_new();
        s.execute("slide.new", &serde_json::json!({})).unwrap();
        let st = s.active().unwrap();
        let (first, second) = (st.doc.slides[0].id, st.doc.slides[1].id);
        assert_eq!(st.selection.slide, 1, "the new slide is shown");
        let title = st.doc.slides[1].shapes[0].id;
        let typing = |slide, notes| {
            let mut place = SlidePlace { slide, selection: Default::default() };
            place.selection.text = Some(deckcraft_engine::TextSel { shape: title, anchor: (0, 0), caret: (0, 0), notes, ..Default::default() });
            RemotePeer { name: "Ann".into(), color: 5, place: Some(place), typing: true }
        };
        let c = text_carets(st, &[typing(second, false), typing(second, true), typing(first, false)]);
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].0, c[0].1.shape), (5, title));
    }

    #[test]
    fn initials_are_the_first_and_last_words() {
        assert_eq!(initials("Ann Lee"), "AL");
        assert_eq!(initials("bob"), "B");
        assert_eq!(initials("Mary Ann van Dyke"), "MD");
        assert_eq!(initials("حسام طه"), "حط");
        assert_eq!(initials("  "), "");
    }
}
