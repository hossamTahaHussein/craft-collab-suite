//! Suggesting: every suggestable command is rejected back to the presentation it started from (the
//! Reject All proof per command), accepted as it is, per person, through undo, files and replays.

use serde_json::{Value, json};

use super::{SUGGEST_COMMANDS, suggestions};
use crate::Session;

/// A deck of two slides; the first has a text box "Hello world", selected.
fn deck() -> Session {
    let mut s = Session::with_new();
    s.prefs.author = "Ann".into();
    s.execute("insert.textBox", &json!({"rect": [100, 100, 300, 80], "text": "Hello world"})).unwrap();
    s.execute("text.exit", &json!({})).unwrap();
    s.execute("slide.new", &json!({})).unwrap();
    s.execute("slide.go", &json!({"index": 0})).unwrap();
    let id = box_id(&s);
    s.execute("edit.select", &json!({"ids": [id]})).unwrap();
    s
}

fn box_id(s: &Session) -> u32 {
    let d = s.active().unwrap();
    d.doc.slides[0].shapes.iter().find(|x| x.text.as_ref().is_some_and(|t| t.text().contains("Hello"))).map(|x| x.id.0).unwrap()
}

fn doc(s: &Session) -> std::sync::Arc<deckcraft_model::Presentation> {
    s.active().unwrap().doc.clone()
}

/// What a command needs first (not suggested): editing text, a caret to delete before.
fn setup(s: &mut Session, id: &str) {
    if matches!(id, "text.insert" | "text.delete" | "text.compose" | "text.commit" | "text.deleteSurrounding") {
        s.execute("text.edit", &json!({"end": true})).unwrap();
    }
    let first = match id {
        "format.clear" => "format.bold",
        "format.outdent" => "format.indent",
        "format.shrink" => "format.grow",
        _ => return,
    };
    s.execute(first, &json!({})).unwrap();
}

fn params(s: &Session, id: &str) -> Value {
    let a = box_id(s);
    match id {
        "text.insert" => json!({"text": "!"}),
        "text.compose" => json!({"text": "に"}),
        "text.commit" => json!({"text": "日本"}),
        "text.deleteSurrounding" => json!({"before": 2, "after": 0}),
        "text.set" => json!({"id": a, "text": "Bonjour"}),
        "format.font" => json!({"family": "Georgia"}),
        "format.size" => json!({"size": 40}),
        "format.color" => json!({"color": "#C00000"}),
        "format.highlight" => json!({"color": "#FFFF00"}),
        "format.case" => json!({"mode": "upper"}),
        "shape.insert" => json!({"preset": "rect", "rect": [500, 300, 100, 80]}),
        "insert.textBox" => json!({"rect": [500, 300, 200, 60], "text": "New"}),
        "shape.move" => json!({"ids": [a], "dx": 40}),
        "shape.resize" => json!({"ids": [a], "w": 360}),
        "shape.setBounds" => json!({"ids": [a], "x": 10, "y": 10, "w": 200, "h": 50}),
        "shape.rotate" => json!({"ids": [a], "deg": 30}),
        "shape.flip" => json!({"ids": [a], "axis": "vertical"}),
        "shape.fill" => json!({"ids": [a], "color": "#00ff00"}),
        "shape.line" => json!({"ids": [a], "color": "#0000ff"}),
        "edit.delete" => json!({"ids": [a]}),
        "slide.layout" => json!({"layout": "Two Content"}),
        _ => json!({}),
    }
}

/// `after` is `before` again, but for the ids the edit used up.
fn as_it_was(before: &deckcraft_model::Presentation, after: &deckcraft_model::Presentation) -> bool {
    deckcraft_model::Presentation { next_id: before.next_id, ..after.clone() } == *before
}

#[test]
fn rejecting_every_suggestable_command_puts_the_presentation_back() {
    for id in SUGGEST_COMMANDS {
        let mut s = deck();
        setup(&mut s, id);
        let before = doc(&s);
        s.suggesting = true;
        let p = params(&s, id);
        s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(*doc(&s) != *before, "{id} changed the presentation");
        assert!(!suggestions(&doc(&s)).is_empty(), "{id} was recorded as a suggestion");
        s.suggesting = false;
        let n = s.execute("review.rejectAllSuggestions", &json!({})).unwrap()["count"].as_u64().unwrap();
        assert!(n > 0, "{id}");
        assert!(as_it_was(&before, &doc(&s)), "{id}: rejected, the presentation is as it was");
    }
}

#[test]
fn accepting_keeps_the_edit_and_rejecting_a_new_shape_deletes_it() {
    let mut s = deck();
    let a = box_id(&s);
    s.suggesting = true;
    s.execute("text.set", &json!({"id": a, "text": "Bonjour"})).unwrap();
    s.execute("shape.insert", &json!({"preset": "ellipse", "rect": [500, 300, 80, 80]})).unwrap();
    let list = suggestions(&doc(&s));
    assert_eq!(list.len(), 2, "{list:?}");
    assert!(list.iter().all(|x| x.author == "Ann"));
    s.execute("edit.select", &json!({"ids": [a]})).unwrap();
    s.execute("review.acceptSuggestion", &json!({})).unwrap();
    assert_eq!(suggestions(&doc(&s)).len(), 1);
    s.execute("review.rejectAllSuggestions", &json!({})).unwrap();
    let d = doc(&s);
    assert_eq!(d.slides[0].shapes.iter().filter(|x| x.text.as_ref().is_some_and(|t| t.text() == "Bonjour")).count(), 1, "accepted text stays");
    assert!(
        !d.slides[0].shapes.iter().any(|x| matches!(&x.geom, deckcraft_model::Geom::Preset { name, .. } if name == "ellipse")),
        "the suggested ellipse is gone"
    );
}

#[test]
fn each_persons_suggestions_are_reviewed_on_their_own() {
    let mut s = deck();
    s.suggesting = true;
    s.execute("shape.move", &json!({"ids": [box_id(&s)], "dx": 50})).unwrap();
    s.prefs.author = "Bob".into();
    s.execute("slide.new", &json!({})).unwrap();
    s.suggesting = false;
    let before_bob = doc(&s).slides.len();
    assert_eq!(s.execute("review.rejectAllSuggestions", &json!({"author": "Bob"})).unwrap()["count"], 1);
    assert_eq!(doc(&s).slides.len(), before_bob - 1, "Bob's new slide is gone");
    let list = suggestions(&doc(&s));
    assert_eq!(list.iter().map(|x| x.author.as_str()).collect::<Vec<_>>(), ["Ann"], "Ann's move stays suggested");
}

#[test]
fn a_deleted_slide_comes_back_in_its_place() {
    let mut s = deck();
    let before = doc(&s);
    s.suggesting = true;
    s.execute("slide.go", &json!({"index": 0})).unwrap();
    s.execute("slide.delete", &json!({})).unwrap();
    assert_eq!(doc(&s).slides.len(), 1);
    s.execute("review.rejectAllSuggestions", &json!({})).unwrap();
    assert!(as_it_was(&before, &doc(&s)));
}

#[test]
fn moving_slides_and_theme_changes_cant_be_suggested() {
    let mut s = deck();
    let before = doc(&s);
    s.suggesting = true;
    for (id, p) in [("slide.move", json!({"from": 0, "to": 1})), ("design.theme", json!({"name": "Ember"}))] {
        let e = s.execute(id, &p).unwrap_err();
        assert!(e.to_string().contains("can't be suggested"), "{id}: {e}");
    }
    assert!(*doc(&s) == *before);
    s.execute("slide.go", &json!({"index": 1})).unwrap();
}

#[test]
fn undo_takes_a_suggestion_back_with_its_record() {
    let mut s = deck();
    let before = doc(&s);
    s.suggesting = true;
    s.execute("format.bold", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(as_it_was(&before, &doc(&s)));
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(suggestions(&doc(&s)).len(), 1);
}

#[test]
fn suggestions_are_saved_in_pptx_and_deckcraft_files() {
    let mut s = deck();
    s.suggesting = true;
    s.execute("shape.rotate", &json!({"ids": [box_id(&s)], "deg": 15})).unwrap();
    for format in ["pptx", "deckcraft"] {
        let bytes = crate::cmd::file::save_bytes(&doc(&s), format).unwrap();
        let back = crate::cmd::file::open_presentation(&format!("deck.{format}"), &bytes).unwrap_or_else(|e| panic!("{format}: {e}"));
        let list = suggestions(&back);
        assert_eq!((list.len(), list[0].author.as_str()), (1, "Ann"), "{format}");
    }
}

#[test]
fn a_replayed_suggestion_is_recorded_alike_on_every_replica() {
    let ctx = crate::collab::RunCtx { author: "Cara".into(), time: 1_700_000_000, track: true, ..Default::default() };
    let (mut a, mut b) = (deck(), deck());
    let id = box_id(&a);
    for s in [&mut a, &mut b] {
        s.set_collab(true);
        s.set_recording(false);
        s.execute_with(&ctx, "shape.move", &json!({"ids": [id], "dy": 20})).unwrap();
    }
    assert!(*doc(&a) == *doc(&b));
    let list = suggestions(&doc(&a));
    assert_eq!((list.len(), list[0].author.as_str(), list[0].time), (1, "Cara", 1_700_000_000));
    a.set_recording(true);
    a.set_collab_role(crate::collab::CollabRole::Suggest);
    assert!(a.execute("slide.move", &json!({"from": 0, "to": 1})).is_err());
    a.execute("shape.move", &json!({"ids": [id], "dx": 5})).unwrap();
    let out = a.take_outbox();
    assert_eq!((out.len(), out[0].ctx.track), (1, true), "a suggester's edit goes out as a suggestion");
}

/// Review finding 3: comments are made while suggesting and by a suggester.
#[test]
fn comments_are_made_while_suggesting() {
    let mut s = deck();
    s.suggesting = true;
    s.execute("comment.add", &json!({"text": "Why?"})).unwrap();
    assert!(suggestions(&doc(&s)).is_empty(), "not a suggestion");
    assert_eq!(doc(&s).slides[0].comments.len(), 1);
}

/// Review finding 4: typing through an input method is a suggestion too; a cancelled composition
/// suggests nothing.
#[test]
fn input_method_typing_is_suggested() {
    let mut s = deck();
    let before = doc(&s);
    s.suggesting = true;
    s.execute("text.edit", &json!({"end": true})).unwrap();
    s.execute("text.compose", &json!({"text": "に"})).unwrap();
    s.execute("text.compose", &json!({"text": ""})).unwrap();
    assert!(suggestions(&doc(&s)).is_empty(), "cancelled: nothing suggested");
    s.execute("text.compose", &json!({"text": "にほん"})).unwrap();
    s.execute("text.commit", &json!({"text": "日本"})).unwrap();
    assert_eq!(suggestions(&doc(&s)).len(), 1);
    s.execute("text.exit", &json!({})).unwrap();
    s.suggesting = false;
    s.execute("review.rejectAllSuggestions", &json!({})).unwrap();
    assert!(as_it_was(&before, &doc(&s)));
}

/// Review finding 5: in a deck with sections, slides are added, duplicated and deleted as
/// suggestions, and rejecting puts the sections back.
#[test]
fn slides_in_sections_are_suggested_and_rejected() {
    let mut s = deck();
    s.execute("section.add", &json!({"name": "Intro", "at": 0})).unwrap();
    s.execute("section.add", &json!({"name": "Body", "at": 1})).unwrap();
    let before = doc(&s);
    s.suggesting = true;
    for (id, p) in [("slide.new", json!({})), ("slide.duplicate", json!({})), ("slide.delete", json!({}))] {
        s.execute("slide.go", &json!({"index": 0})).unwrap();
        s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    assert_eq!(suggestions(&doc(&s)).len(), 3);
    s.suggesting = false;
    s.execute("review.rejectAllSuggestions", &json!({})).unwrap();
    assert!(as_it_was(&before, &doc(&s)), "slides and sections as they were");
}

/// Review finding 6: rejecting a suggested slide setting keeps others' later notes and comments.
#[test]
fn rejecting_a_setting_keeps_later_notes_and_comments() {
    let mut s = deck();
    s.suggesting = true;
    s.execute("slide.hide", &json!({"index": 0})).unwrap();
    s.suggesting = false;
    s.execute("slide.notes", &json!({"index": 0, "text": "Say hello"})).unwrap();
    s.execute("comment.add", &json!({"text": "Bob's"})).unwrap();
    s.execute("review.rejectAllSuggestions", &json!({})).unwrap();
    let sl = &doc(&s).slides[0];
    assert!(!sl.hidden, "the suggestion was rejected");
    assert_eq!(sl.notes.text(), "Say hello");
    assert_eq!(sl.comments.len(), 1);
}

/// Review finding 7: a direct edit of a suggested shape settles its suggestion.
#[test]
fn a_direct_edit_settles_the_suggestion() {
    let mut s = deck();
    let a = box_id(&s);
    s.suggesting = true;
    s.execute("shape.move", &json!({"ids": [a], "dx": 40})).unwrap();
    s.suggesting = false;
    s.execute("shape.rotate", &json!({"ids": [a], "deg": 10})).unwrap();
    assert!(suggestions(&doc(&s)).is_empty(), "settled");
}

/// Review finding 8: Reject with a shape selected that has no suggestion rejects nothing.
#[test]
fn reject_with_an_unsuggested_shape_selected_does_nothing() {
    let mut s = deck();
    s.suggesting = true;
    s.execute("shape.insert", &json!({"preset": "rect", "rect": [500, 300, 100, 80]})).unwrap();
    s.suggesting = false;
    s.execute("edit.select", &json!({"ids": [box_id(&s)]})).unwrap();
    assert_eq!(s.execute("review.rejectSuggestion", &json!({})).unwrap()["count"], 0);
    assert_eq!(suggestions(&doc(&s)).len(), 1);
}

/// Review findings 13 and 16: a refused suggestion leaves no undo step; an unreadable record is
/// kept.
#[test]
fn a_refused_suggestion_leaves_no_undo_step_and_unknown_records_are_kept() {
    let mut s = deck();
    let undo = s.active().unwrap().history.undo.len();
    s.suggesting = true;
    assert!(s.execute("design.theme", &json!({"name": "Ember"})).is_err());
    assert_eq!(s.active().unwrap().history.undo.len(), undo);
    {
        let d = s.active_mut().unwrap();
        let mut p = (*d.doc).clone();
        p.tags.push((super::TAG.to_string(), r#"[{"before":{"kind":"fromTheFuture"},"author":"X","time":1}]"#.to_string()));
        d.doc = std::sync::Arc::new(p);
    }
    s.execute("shape.move", &json!({"ids": [box_id(&s)], "dx": 5})).unwrap();
    let d = doc(&s);
    let stored = &d.tags.iter().find(|(k, _)| k == super::TAG).unwrap().1;
    assert!(stored.contains("fromTheFuture"), "kept: {stored}");
    assert_eq!(suggestions(&d).len(), 1);
}
