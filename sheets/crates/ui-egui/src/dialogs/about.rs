use super::Dialog;

/// Help ▸ About Craft Sheets: the app, its contributors and the AI models that helped. The credits
/// are compiled in (`crate::credits`, docs/contributors.md).
pub(super) fn about(ui: &mut egui::Ui, d: &mut Dialog) {
    let lang = crate::i18n::current(ui.ctx());
    crate::rtl::horizontal(ui, |ui| {
        for tab in ["About", "Contributors", "Models"] {
            if ui.selectable_label(d.tab == tab, lang.tr(tab)).clicked() {
                d.tab = tab.to_string();
            }
        }
    });
    ui.separator();
    match d.tab.as_str() {
        "Contributors" => crate::credits::contributors_ui(ui),
        "Models" => crate::credits::models_ui(ui),
        _ => crate::rtl::body(ui, |ui| {
            ui.heading("Craft Sheets");
            ui.label(lang.tr("Version {}").replace("{}", env!("CARGO_PKG_VERSION")));
            ui.add_space(4.0);
            ui.label(lang.tr("A clean-room, open-source, Rust-native spreadsheet."));
            ui.label(lang.tr("Part of Craft Collab Suite."));
            ui.add_space(4.0);
            ui.label(egui::RichText::new(lang.tr("Craft Sheets by Hossam Taha")).strong());
            ui.hyperlink_to("hossamtaha.me", "https://hossamtaha.me");
            ui.hyperlink_to(lang.tr("Source on GitHub"), "https://github.com/hossamTahaHussein/craft-collab-suite");
            ui.hyperlink_to(lang.tr("Discussions on GitHub"), "https://github.com/hossamTahaHussein/craft-collab-suite/discussions");
            ui.label(lang.tr("Based on GridCraft by the ArtCraft team."));
        }),
    }
}
