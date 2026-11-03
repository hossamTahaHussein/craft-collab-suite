//! Co-editing mode (`crate::collab`): the person's edits are recorded once, with the place and
//! context they ran in; replaying one under that context gives the same deck anywhere, and can't
//! reach this computer's files.

use serde_json::json;

use crate::Session;
use crate::collab::RunCtx;

fn s() -> Session {
    let mut s = Session::with_new();
    s.set_collab(true);
    s
}

#[test]
fn the_outbox_holds_each_edit_that_changed_the_deck_once() {
    let mut s = s();
    s.execute("slide.new", &json!({})).unwrap();
    s.execute("view.zoom", &json!({"value": 150})).ok();
    s.execute("slide.inspect", &json!({})).ok();
    let out = s.take_outbox();
    assert_eq!(out.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["slide.new"], "{out:?}");
    assert!(out[0].place.is_some());
    assert!(s.take_outbox().is_empty(), "taken");
}

#[test]
fn undo_and_redo_go_to_the_co_editing_layer() {
    let mut s = s();
    s.execute("slide.new", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().doc.slides.len(), 2, "not undone on this session's own history");
    let ids: Vec<String> = s.take_outbox().into_iter().map(|r| r.id).collect();
    assert_eq!(ids, ["slide.new", "edit.undo"]);
}

#[test]
fn a_comment_replayed_anywhere_carries_its_authors_time() {
    let ctx = RunCtx { author: "Ann".into(), time: 1_800_000_000, ..RunCtx::default() };
    let run = || {
        let mut s = s();
        s.set_recording(false);
        s.execute_with(&ctx, "comment.add", &json!({"text": "note"})).unwrap();
        s.doc().unwrap().doc.clone()
    };
    let (a, b) = (run(), run());
    assert_eq!(a, b);
    let text = serde_json::to_string(&*a).unwrap();
    assert!(text.contains("2027-01-15"), "the edit's time, not this computer's");
}

#[test]
fn what_co_editing_cant_do_is_refused_live_and_replayed() {
    let mut s = s();
    for cmd in ["file.new", "file.open", "file.close", "review.compare"] {
        assert!(s.execute(cmd, &json!({"path": "/etc/hostname"})).unwrap_err().to_string().contains("co-editing"), "{cmd}");
        assert!(s.execute_with(&RunCtx::default(), cmd, &json!({"path": "/etc/hostname"})).is_err(), "{cmd} replayed");
    }
    assert!(s.execute_with(&RunCtx::default(), "edit.undo", &json!({})).is_err());
    assert!(s.take_outbox().is_empty());
}

#[test]
fn a_replayed_edit_cant_read_this_computers_files() {
    let png = std::env::temp_dir().join(format!("deckcraft-replay-{}.png", std::process::id()));
    let bytes: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68,
        65, 84, 120, 218, 99, 100, 96, 248, 95, 15, 0, 2, 135, 1, 128, 235, 71, 186, 146, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    std::fs::write(&png, bytes).unwrap();
    let mut plain = Session::with_new();
    assert!(plain.execute("insert.picture", &json!({"path": png.to_str().unwrap()})).is_ok(), "the picture itself is fine");
    let mut s = s();
    s.set_recording(false);
    let r = s.execute_with(&RunCtx::default(), "insert.picture", &json!({"path": png.to_str().unwrap()}));
    let _ = std::fs::remove_file(&png);
    assert!(r.is_err(), "a replayed picture from a local path: {r:?}");
}

#[test]
fn a_replayed_edit_is_not_recorded_again() {
    let mut s = s();
    s.set_recording(false);
    s.execute_with(&RunCtx::default(), "slide.new", &json!({})).unwrap();
    assert!(s.take_outbox().is_empty());
    assert_eq!(s.doc().unwrap().doc.slides.len(), 2);
}

#[test]
fn an_edit_runs_on_the_slide_it_was_made_on() {
    let mut a = s();
    a.execute("slide.new", &json!({})).unwrap();
    let place = a.collab_place().unwrap();
    a.execute("slide.new", &json!({})).unwrap();
    a.set_collab_place(&place);
    assert_eq!(a.doc().unwrap().selection.slide, 1, "the slide by its id");
    let mut gone = place.clone();
    gone.slide = deckcraft_model::SlideId(999_999);
    a.set_collab_place(&gone);
    assert_eq!(a.doc().unwrap().selection.slide, 0, "a slide that is gone gives the first");
}
