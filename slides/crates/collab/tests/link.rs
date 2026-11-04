//! The page ↔ replica glue on a presentation (`deckcraft_collab::link`).

use craft_collab_core::doc::CollabDoc;
use deckcraft_collab::SlidesDoc;
use deckcraft_collab::link::CollabLink;
use deckcraft_engine::Session;
use serde_json::{Value as Json, json};

fn parse(m: &str) -> Json {
    serde_json::from_str(m).unwrap()
}

fn first() -> (CollabLink, Session) {
    let mut s = Session::new();
    let mut link = CollabLink::new(Some("deck.pptx".into()));
    let welcome = json!({"t": "Welcome", "rev": 0, "snapshot": null, "ops": [], "you": "me"}).to_string();
    assert!(link.on_message(&welcome, &mut s).is_empty());
    s.execute("file.new", &Json::Null).unwrap();
    let out = link.tick(&mut s, 0.0);
    assert!(out.iter().any(|m| parse(m)["t"] == "Snapshot"), "{out:?}");
    assert!(link.take_document_ready());
    (link, s)
}

#[test]
fn an_edit_goes_out_and_a_joiner_gets_it() {
    let (mut link, mut s) = first();
    let snapshot = SlidesDoc::encode(SlidesDoc::doc(&s)).unwrap();
    s.execute("slide.new", &json!({})).unwrap();
    let ops: Vec<Json> = link.tick(&mut s, 10.0).iter().map(|m| parse(m)).filter(|m| m["t"] == "Op").collect();
    assert_eq!(ops.len(), 1);
    let mut op = ops[0].clone();
    op["rev"] = json!(1);
    let mut j = Session::new();
    let mut joiner = CollabLink::new(None);
    joiner.on_message(&json!({"t": "Welcome", "rev": 0, "snapshot": snapshot, "ops": [], "you": "b"}).to_string(), &mut j);
    assert!(joiner.take_document_ready());
    assert_eq!(j.doc().unwrap().doc.slides.len(), 1);
    joiner.on_message(&op.to_string(), &mut j);
    assert_eq!(j.doc().unwrap().doc.slides.len(), 2);
}

#[test]
fn a_viewer_cant_edit() {
    let mut s = Session::new();
    s.execute("file.new", &Json::Null).unwrap();
    let snapshot = SlidesDoc::encode(SlidesDoc::doc(&s)).unwrap();
    let mut v = Session::new();
    let mut link = CollabLink::new(None);
    link.on_message(&json!({"t": "Welcome", "rev": 0, "snapshot": snapshot, "ops": [], "you": "v", "role": "view"}).to_string(), &mut v);
    assert!(v.execute("slide.new", &json!({})).is_err());
    assert!(link.tick(&mut v, 0.0).iter().all(|m| parse(m)["t"] != "Op"));
}
