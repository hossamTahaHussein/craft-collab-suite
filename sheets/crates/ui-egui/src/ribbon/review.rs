use super::controls::{act, anchor_keytip, control_keytip, menu_items, sep};
use super::{Icon, SheetApp, Stroke, StrokeKind, Tokens, Ui, big_button, json, small_button};

pub(super) fn review(app: &mut SheetApp, ui: &mut Ui) {
    let __k = big_button(ui, Icon::Spell, "Spelling", "Spelling (F7)", false);
    control_keytip(app, ui, &__k, "dialog:spelling");
    if __k.clicked() {
        app.open_dialog("spelling", json!({}));
    }
    let __k = big_button(ui, Icon::Calc, "Workbook\nStatistics", "Workbook Statistics", false);
    control_keytip(app, ui, &__k, "statistics");
    if __k.clicked() {
        app.open_dialog("statistics", json!({}));
    }
    let __k = big_button(ui, Icon::Validation, "Check\nAccessibility", "Check Accessibility", false);
    control_keytip(app, ui, &__k, "dialog:accessibility");
    if __k.clicked() {
        app.open_dialog("accessibility", json!({}));
    }
    sep(ui);
    let __k = big_button(ui, Icon::Comment, "New\nComment", "New Comment", false);
    control_keytip(app, ui, &__k, "dialog:comment");
    if __k.clicked() {
        app.open_dialog("comment", json!({"threaded": true}));
    }
    let __k = big_button(ui, Icon::Delete, "Delete", "Delete Comment", false);
    control_keytip(app, ui, &__k, "review.deleteComment");
    if __k.clicked() {
        act(app, "review.deleteComment", json!({}));
    }
    crate::rtl::vertical(ui, |ui| {
        for (icon, label, tip, id) in
            [(Icon::Left, "Previous", "Previous Comment", "review.previousComment"), (Icon::Right, "Next", "Next Comment", "review.nextComment")]
        {
            if small_button(ui, icon, label, tip, false).clicked() {
                act(app, id, json!({}));
                app.grid.ensure_visible = true;
            }
        }
    });
    let shown = app.session.comments_pane;
    let r = big_button(ui, Icon::Comment, "Show\nComments", "Comments pane", false);
    if shown {
        ui.painter().rect_stroke(r.rect, 5.0, Stroke::new(2.0, Tokens::get(ui.ctx()).accent), StrokeKind::Inside);
    }
    if r.clicked() {
        act(app, "review.showComments", json!({"on": !shown}));
        if app.session.comments_pane {
            app.grid.pane = None;
        }
    }
    let notes = big_button(ui, Icon::Note, "Notes", "Notes", true);
    anchor_keytip(app, ui, &notes, &["review.nextComment"]);
    egui::Popup::menu(&notes).show(|ui| {
        menu_items(
            app,
            ui,
            &[
                ("New Note", "dialog:comment", json!({"threaded": false})),
                ("Show/Hide Note", "review.showNote", json!({})),
                ("Show All Notes", "review.showAllNotes", json!({})),
                ("-", "", json!({})),
                ("Convert to Comments", "review.convertToComments", json!({})),
            ],
        );
    });
    sep(ui);
    suggesting(app, ui);
    sep(ui);
    let protected = app.active_sheet_protected();
    let __k = big_button(ui, Icon::Lock, if protected { "Unprotect\nSheet" } else { "Protect\nSheet" }, "Protect Sheet", false);
    control_keytip(app, ui, &__k, "dialog:protectSheet");
    if __k.clicked() {
        app.open_dialog("protectSheet", json!({}));
    }
    let __k = big_button(ui, Icon::Book, "Protect\nWorkbook", "Protect Workbook structure", false);
    control_keytip(app, ui, &__k, "review.protectWorkbook");
    if __k.clicked() {
        crate::backstage::protect_workbook(app);
    }
    if big_button(ui, Icon::Validation, "Allow Edit\nRanges", "Allow Edit Ranges", false).clicked() {
        app.open_dialog("allowEditRanges", json!({}));
    }
}

/// Suggesting: this person's edits become suggestions, and the buttons that accept or reject
/// them (the active cell's, or everyone's from the menu).
fn suggesting(app: &mut SheetApp, ui: &mut Ui) {
    let suggester = app.session.collab_role() == gridcraft_engine::collab::CollabRole::Suggest;
    let on = app.session.suggesting || suggester;
    let r = big_button(ui, Icon::Pen, "Suggesting", "Suggest changes that others accept or reject", false);
    if on {
        ui.painter().rect_stroke(r.rect, 5.0, Stroke::new(2.0, Tokens::get(ui.ctx()).accent), StrokeKind::Inside);
    }
    control_keytip(app, ui, &r, "review.suggesting");
    // A suggester's edits are always suggestions.
    if r.clicked() && !suggester {
        act(app, "review.suggesting", json!({"on": !on}));
    }
    crate::rtl::vertical(ui, |ui| {
        for (icon, label, tip, id) in [
            (Icon::Check, "Accept", "Accept the suggestion in this cell", "review.acceptSuggestion"),
            (Icon::Close, "Reject", "Reject the suggestion in this cell", "review.rejectSuggestion"),
        ] {
            if small_button(ui, icon, label, tip, false).clicked() {
                act(app, id, json!({}));
            }
        }
    });
    let all = big_button(ui, Icon::More, "All\nSuggestions", "Accept or reject every suggestion, or one person's", true);
    egui::Popup::menu(&all).show(|ui| {
        menu_items(
            app,
            ui,
            &[
                ("Accept All Suggestions", "review.acceptAllSuggestions", json!({})),
                ("Reject All Suggestions", "review.rejectAllSuggestions", json!({})),
            ],
        );
        let mut people: Vec<String> =
            app.session.active().map(|d| gridcraft_engine::suggest::suggestions(&d.wb).into_iter().map(|x| x.author).collect()).unwrap_or_default();
        people.sort();
        people.dedup();
        for person in people {
            ui.separator();
            let lang = crate::i18n::current(ui.ctx());
            for (label, id) in
                [("Accept {}'s Suggestions", "review.acceptAllSuggestions"), ("Reject {}'s Suggestions", "review.rejectAllSuggestions")]
            {
                if ui.button(lang.tr(label).replace("{}", &person)).clicked() {
                    act(app, id, json!({"author": person}));
                }
            }
        }
    });
}
