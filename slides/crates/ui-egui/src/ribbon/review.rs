use super::{Icon, SlideApp, Ui, Value, big, big_button, big_button_ex, enabled, group, json, rows, run, small_button, tip};

fn comment_nav(app: &mut SlideApp, ui: &mut Ui, forward: bool) {
    let (icon, label, id) = if forward { (Icon::NextComment, "Next", "comment.next") } else { (Icon::PrevComment, "Previous", "comment.prev") };
    if small_button(ui, icon, label, enabled(app, id), false).on_hover_text(tip(id, label)).clicked() {
        let params = match app.ui.comment_focus {
            Some((slide, index)) if app.session.active().is_some_and(|d| d.selection.slide == slide) => json!({"slide": slide, "index": index}),
            _ => json!({}),
        };
        if let Ok(v) = app.run(id, params)
            && let (Some(slide), Some(index)) = (v.get("slide").and_then(Value::as_u64), v.get("index").and_then(Value::as_u64))
            && let (Ok(slide), Ok(index)) = (usize::try_from(slide), usize::try_from(index))
        {
            app.ui.comment_focus = Some((slide, index));
            app.ui.pane = Some("comments".into());
        }
    }
}

pub(super) fn review(app: &mut SlideApp, ui: &mut Ui) {
    group(ui, |ui| {
        big(app, ui, Icon::Spelling, "Spelling", "app.dialog", json!({"id": "spelling"}));
        big(app, ui, Icon::Thesaurus, "Thesaurus", "review.thesaurusPane", json!({}));
    });
    group(ui, |ui| {
        big(app, ui, Icon::Accessibility, "Check\nAccessibility", "app.dialog", json!({"id": "accessibility"}));
    });
    group(ui, |ui| {
        let r = big_button(ui, Icon::Translate, "Translate", false);
        let _ = r.on_hover_text("Translation needs a language model (not bundled)");
        big(app, ui, Icon::Language, "Language", "app.dialog", json!({"id": "language"}));
    });
    group(ui, |ui| {
        big(app, ui, Icon::Compare, "Compare", "app.compareDialog", json!({}));
        let comparing = enabled(app, "review.endCompare");
        if big_button(ui, Icon::Check, "Accept\nAll", comparing).clicked() {
            run(app, "review.accept", json!({"all": true}));
        }
        if big_button(ui, Icon::Close, "End\nReview", comparing).clicked() {
            run(app, "review.endCompare", json!({}));
            if app.ui.pane.as_deref() == Some("revisions") {
                app.ui.pane = None;
            }
        }
    });
    group(ui, |ui| suggesting(app, ui));
    group(ui, |ui| {
        big(app, ui, Icon::NewComment, "New\nComment", "app.dialog", json!({"id": "comment"}));
        rows(ui, |ui| {
            comment_nav(app, ui, false);
            comment_nav(app, ui, true);
        });
        big(app, ui, Icon::DeleteComment, "Delete", "comment.delete", json!({}));
        let show = app.ui.pane.as_deref() == Some("comments");
        if big_button_ex(ui, Icon::ShowComments, "Show\nComments", true, false, show).0.clicked() {
            let _ = app.run("view.pane", json!({"pane": "comments", "toggle": true}));
        }
    });
}

/// Suggesting: this person's edits become suggestions, and the buttons that accept or reject them
/// (the selected shapes', else the slide's; or everyone's, or one person's, from the menu).
fn suggesting(app: &mut SlideApp, ui: &mut Ui) {
    let suggester = app.session.collab_role() == deckcraft_engine::collab::CollabRole::Suggest;
    let on = app.session.suggesting || suggester;
    let r = big_button_ex(ui, Icon::Pen, "Suggesting", !suggester, false, on).0;
    if r.on_hover_text(crate::i18n::tr("Suggest changes that others accept or reject")).clicked() {
        run(app, "review.suggesting", json!({"on": !on}));
    }
    // Reviewing is for people who may edit, as in Craft Docs.
    if suggester {
        return;
    }
    let (any, here) = app.session.active().map_or((false, false), |d| {
        let list = deckcraft_engine::suggest::cached(&d.doc);
        (!list.is_empty(), !deckcraft_engine::suggest::reachable(&d.doc, &d.selection, &list).is_empty())
    });
    rows(ui, |ui| {
        for (icon, label, id) in [(Icon::Check, "Accept", "review.acceptSuggestion"), (Icon::Close, "Reject", "review.rejectSuggestion")] {
            if small_button(ui, icon, label, here, false).on_hover_text(tip(id, label)).clicked() {
                run(app, id, json!({}));
            }
        }
    });
    let (all, _) = big_button_ex(ui, Icon::Check, "All\nSuggestions", any, true, false);
    egui::Popup::menu(&all).show(|ui| {
        let tr = crate::i18n::tr;
        if ui.button(tr("Accept All Suggestions")).clicked() {
            run(app, "review.acceptAllSuggestions", json!({}));
        }
        if ui.button(tr("Reject All Suggestions")).clicked() {
            run(app, "review.rejectAllSuggestions", json!({}));
        }
        let mut people: Vec<String> =
            app.session.active().map(|d| deckcraft_engine::suggest::cached(&d.doc).iter().map(|x| x.author.clone()).collect()).unwrap_or_default();
        people.sort();
        people.dedup();
        for person in people {
            ui.separator();
            for (label, id) in
                [("Accept {}'s Suggestions", "review.acceptAllSuggestions"), ("Reject {}'s Suggestions", "review.rejectAllSuggestions")]
            {
                if ui.button(tr(label).replace("{}", &person)).clicked() {
                    run(app, id, json!({"author": person}));
                }
            }
        }
    });
}
