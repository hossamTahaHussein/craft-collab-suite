//! The interface direction for laying out the chrome: right to left in Arabic, as Craft Docs
//! draws it. `i18n::install` sets it each frame from the interface language ([`set`]); layouts ask
//! [`dir`] for it, so a row starts at the reading start wherever it is built. Slides keep their
//! own geometry: only the chrome mirrors.

use std::cell::Cell;

use craft_ui::direction::Direction;

thread_local! {
    /// The direction of the interface this thread draws (one UI thread; each test its own).
    static RTL: Cell<bool> = const { Cell::new(false) };
}

/// The interface direction now.
pub fn dir() -> Direction {
    if RTL.with(Cell::get) { Direction::Rtl } else { Direction::Ltr }
}

/// Whether the interface reads right to left now.
pub fn is_rtl() -> bool {
    RTL.with(Cell::get)
}

/// Draw the interface right to left (`true`) or left to right, on `ctx` and for [`dir`].
pub fn set(ctx: &egui::Context, rtl: bool) {
    RTL.with(|c| c.set(rtl));
    if craft_ui::direction::is_rtl(ctx) != rtl {
        craft_ui::direction::apply(ctx, if rtl { Direction::Rtl } else { Direction::Ltr });
    }
}

/// A vertically centred row in reading order.
pub fn row() -> egui::Layout {
    craft_ui::direction::row(&dir())
}

/// A vertically centred row from the reading end (a bar's far-end group, a dialog's buttons).
pub fn trailing() -> egui::Layout {
    craft_ui::direction::trailing(&dir())
}

/// [`egui::Ui::horizontal`] in reading order.
pub fn horizontal<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    let size = egui::vec2(ui.available_size_before_wrap().x, ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, row(), add)
}

/// [`egui::Ui::horizontal_centered`] in reading order: the rest of the space, centred.
pub fn horizontal_centered<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    let size = ui.available_size_before_wrap();
    ui.allocate_ui_with_layout(size, row(), add)
}

/// [`egui::Ui::horizontal_wrapped`] in reading order.
pub fn horizontal_wrapped<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    let size = egui::vec2(ui.available_size_before_wrap().x, ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, row().with_main_wrap(true), add)
}

/// An anchor written for left to right, mirrored in a right-to-left interface.
pub fn align2(ltr: egui::Align2) -> egui::Align2 {
    craft_ui::direction::align2(&dir(), ltr)
}

/// [`egui::Ui::vertical`] against the reading start: right to left its items hug the right, so a
/// ribbon group's column sits beside the one before it.
pub fn vertical<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    if !is_rtl() {
        return ui.vertical(add);
    }
    ui.with_layout(egui::Layout::top_down(egui::Align::Max), add)
}

/// A row as wide as its content (as it was last frame), at the reading start of a column: right to
/// left a column's rows of small buttons end at its right edge, with their own order kept (as in
/// Craft Docs, a group's grid keeps its left-to-right order in place).
pub fn fit_row<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    if !is_rtl() {
        return ui.horizontal(add);
    }
    let id = ui.next_auto_id().with("rtl_fit_row");
    let last: Option<f32> = ui.ctx().data(|d| d.get_temp(id));
    let width = last.unwrap_or_else(|| ui.available_size_before_wrap().x).min(ui.available_size_before_wrap().x);
    let r = ui.allocate_ui_with_layout(egui::vec2(width, ui.spacing().interact_size.y), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        let r = add(ui);
        (r, ui.min_rect().width())
    });
    let (inner, used) = r.inner;
    if last.is_none_or(|w| (w - used).abs() > 0.5) {
        ui.ctx().data_mut(|d| d.insert_temp(id, used));
        ui.ctx().request_discard("a right-to-left row learnt its width");
    }
    egui::InnerResponse::new(inner, r.response)
}

/// The rect of size `size` the next widget of a row takes, at the cursor in reading order
/// (right to left, it ends at the cursor), for code that paints there before allocating it.
pub fn at_cursor(ui: &egui::Ui, size: egui::Vec2) -> egui::Rect {
    let at = ui.cursor();
    if ui.layout().prefer_right_to_left() {
        egui::Rect::from_min_size(egui::pos2(at.max.x - size.x, at.min.y), size)
    } else {
        egui::Rect::from_min_size(at.min, size)
    }
}

/// A top-down body against the reading start (a dialog's, the File page's): right to left, labels
/// and fields end at the right. Left to right, `add` runs on `ui` itself.
pub fn body<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if !is_rtl() {
        return add(ui);
    }
    ui.with_layout(egui::Layout::top_down(egui::Align::Max), add).inner
}

/// [`egui::Ui::columns`] in reading order: right to left the first column is the rightmost.
pub fn columns<R>(ui: &mut egui::Ui, n: usize, add: impl FnOnce(&mut [egui::Ui]) -> R) -> R {
    ui.columns(n, |cols| {
        if is_rtl() {
            cols.reverse();
        }
        add(cols)
    })
}

/// `r`, laid out left to right inside `outer`, where it goes in the interface's direction: right
/// to left, mirrored across `outer` (for chrome painted at computed positions).
pub fn place(outer: egui::Rect, r: egui::Rect) -> egui::Rect {
    if !is_rtl() {
        return r;
    }
    egui::Rect::from_min_max(egui::pos2(outer.min.x + outer.max.x - r.max.x, r.min.y), egui::pos2(outer.min.x + outer.max.x - r.min.x, r.max.y))
}

/// The x of `x`, laid out left to right inside `outer`, in the interface's direction.
pub fn place_x(outer: egui::Rect, x: f32) -> f32 {
    if is_rtl() { outer.min.x + outer.max.x - x } else { x }
}

/// Paint `text` starting at `at.x` (left to right, vertically centred on `at.y`), clipped to `clip`,
/// both inside `outer`, where it goes in the interface's direction. Returns the rect as laid out
/// left to right (to place what follows it) and the rect it was drawn in (to hit-test it).
pub fn text_in(
    p: &egui::Painter,
    outer: egui::Rect,
    clip: egui::Rect,
    at: egui::Pos2,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) -> (egui::Rect, egui::Rect) {
    let galley = p.layout_no_wrap(text.to_owned(), font, color);
    let ltr = egui::Rect::from_min_size(egui::pos2(at.x, at.y - galley.size().y / 2.0), galley.size());
    let (shown, clip) = (place(outer, ltr), place(outer, clip));
    p.with_clip_rect(clip.intersect(p.clip_rect())).galley(shown.min, galley, color);
    (ltr, shown.intersect(clip))
}
