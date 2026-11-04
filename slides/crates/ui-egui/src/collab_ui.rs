//! Co-editing UI for a shared presentation (co-editing spec §7, §9), as Craft Docs has it: the
//! save indicator and the role in the title bar, Version history from the status bar (a kept
//! version previews as its slides, read only; editors restore it) and the read-only notice. The
//! windows are `craft_collab_ui`'s; the host fills [`SlideApp::collab`] from its co-editing link.

use std::sync::Arc;

use deckcraft_engine::collab::CollabRole;
use deckcraft_model::Presentation;

pub use craft_collab_ui::{CollabRequest, SyncState, VersionHistory, VersionRow};

use crate::SlideApp;

/// The co-editing state the host keeps up to date each frame.
pub type CollabUi = craft_collab_ui::CollabUi<DeckPreview>;

/// How many slides a version's preview shows.
const PREVIEW_SLIDES: usize = 30;
/// The preview's slide width, in points.
const PREVIEW_WIDTH: f32 = 380.0;

/// A kept version of the presentation, shown as its slides.
pub struct DeckPreview {
    /// `None` when the version is gone or can't be read.
    deck: Option<Arc<Presentation>>,
    /// Its slides, drawn on first show.
    slides: Option<Vec<egui::TextureHandle>>,
}

impl std::fmt::Debug for DeckPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeckPreview").field("deck", &self.deck.is_some()).field("slides", &self.slides.as_ref().map(Vec::len)).finish()
    }
}

impl DeckPreview {
    pub fn new(deck: Option<Arc<Presentation>>) -> DeckPreview {
        DeckPreview { deck, slides: None }
    }

    /// The slides as pictures `width` points wide (the first [`PREVIEW_SLIDES`]).
    fn slides(&mut self, ctx: &egui::Context, width: f32) -> &[egui::TextureHandle] {
        if self.slides.is_none() {
            let mut out = Vec::new();
            if let Some(deck) = &self.deck {
                let scale = f64::from(width * ctx.pixels_per_point()) / deck.slide_size.width.max(1.0);
                for i in 0..deck.slides.len().min(PREVIEW_SLIDES) {
                    let img = deckcraft_render::render_slide(deck, i, &deckcraft_render::RenderOpts { scale, ..Default::default() });
                    let color = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.to_straight());
                    out.push(ctx.load_texture(format!("collab_version_slide_{i}"), color, egui::TextureOptions::LINEAR));
                }
            }
            self.slides = Some(out);
        }
        self.slides.as_deref().unwrap_or_default()
    }
}

impl craft_collab_ui::Preview for DeckPreview {
    fn kept(&self) -> bool {
        self.deck.is_some()
    }

    fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        for (i, tex) in self.slides(&ctx, PREVIEW_WIDTH).iter().enumerate() {
            let size = tex.size_vec2() / ctx.pixels_per_point();
            let r = ui.add(egui::Image::new((tex.id(), size)));
            let label = format!("Kept slide {}", i + 1);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, &label));
            ui.add_space(6.0);
        }
    }
}

impl SlideApp {
    /// Version history while co-editing: open the window and ask the host for the list.
    pub fn open_version_history(&mut self) {
        self.collab.open_history();
    }

    /// The host's co-editing requests since the last call.
    pub fn take_collab_requests(&mut self) -> Vec<CollabRequest> {
        self.collab.take_requests()
    }

    /// Undo and redo while co-editing are this person's own edits for everyone; the host says
    /// whether there is one, and viewers have none.
    pub(crate) fn collab_undo_state(&self) -> Option<(bool, bool)> {
        let viewer = self.session.collab_role() == CollabRole::View;
        self.collab.active.then_some((self.collab.can_undo && !viewer, self.collab.can_redo && !viewer))
    }
}

/// The role's label in the title bar: `None` for an editor.
pub(crate) fn role_label(app: &SlideApp) -> Option<&'static str> {
    if !app.session.collab() {
        return None;
    }
    match app.session.collab_role() {
        CollabRole::Edit => None,
        CollabRole::Comment => Some("Commenting"),
        CollabRole::Suggest => Some("Suggesting"),
        CollabRole::View => Some("View only"),
    }
}

/// The title bar's co-editing words: the save indicator and the role.
pub(crate) fn title_words(app: &SlideApp) -> Vec<&'static str> {
    app.collab.sync.map(SyncState::label).into_iter().chain(role_label(app)).collect()
}

pub(crate) fn show(app: &mut SlideApp, ctx: &egui::Context) {
    let editor = app.session.collab_role() == CollabRole::Edit;
    craft_collab_ui::show(&mut app.collab, ctx, editor, &|s| s.to_string());
}

#[cfg(test)]
mod tests;
