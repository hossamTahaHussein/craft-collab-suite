//! The page ↔ replica glue on a workbook (`gridcraft_collab::link`).

use gridcraft_collab::link::CollabLink;
use gridcraft_core::{CellRef, Value};
use gridcraft_engine::Session;
use serde_json::{Value as Json, json};

fn parse(m: &str) -> Json {
    serde_json::from_str(m).unwrap()
}

fn value(s: &Session, a1: &str) -> Value {
    s.doc().unwrap().wb.active().unwrap().value(CellRef::parse(a1).unwrap())
}

/// The first person: their open workbook becomes revision 0.
fn first() -> (CollabLink, Session) {
    let mut s = Session::new();
    s.new_workbook();
    let mut link = CollabLink::new(Some("budget.xlsx".into()));
    let welcome = json!({"t": "Welcome", "rev": 0, "snapshot": null, "ops": [], "you": "me"}).to_string();
    assert!(link.on_message(&welcome, &mut s).is_empty());
    // The host opens the document the page asked for: here a new one.
    s.new_workbook();
    let out = link.tick(&mut s, 0.0);
    assert!(out.iter().any(|m| parse(m)["t"] == "Snapshot"), "{out:?}");
    assert!(link.take_document_ready());
    (link, s)
}

#[test]
fn an_edit_goes_out_once_and_the_echo_releases_the_next() {
    let (mut link, mut s) = first();
    s.execute("cell.set", json!({"cell": "B2", "input": "5"})).unwrap();
    s.execute("cell.set", json!({"cell": "B3", "input": "6"})).unwrap();
    let out: Vec<Json> = link.tick(&mut s, 10.0).iter().map(|m| parse(m)).filter(|m| m["t"] == "Op").collect();
    assert_eq!(out.len(), 1, "one edit in flight");
    assert_eq!(out[0]["cmd"], "cell.set");
    assert_eq!(out[0]["sel"]["refs"][0][0], "cell");
    let mut echo = out[0].clone();
    echo["rev"] = json!(1);
    link.on_message(&echo.to_string(), &mut s);
    let next: Vec<Json> = link.tick(&mut s, 20.0).iter().map(|m| parse(m)).filter(|m| m["t"] == "Op").collect();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0]["params"]["cell"], "B3");
}

#[test]
fn a_joiner_shows_the_snapshot_and_someone_elses_edit() {
    let (link, s) = first();
    let _ = link;
    let snapshot = gridcraft_collab::adapter::encode_workbook(&s.doc().unwrap().wb).unwrap();
    let mut j = Session::new();
    let mut joiner = CollabLink::new(None);
    let welcome = json!({"t": "Welcome", "rev": 0, "snapshot": snapshot, "ops": [], "you": "b", "role": "edit"}).to_string();
    assert!(joiner.on_message(&welcome, &mut j).is_empty());
    assert!(joiner.take_document_ready());
    let op = json!({"t": "Op", "client": "a", "seq": 1, "base": 0, "kind": "cmd", "cmd": "cell.set",
        "params": {"cell": "A1", "input": "hi"}, "sel": {"sheet": "Sheet1", "selection": {"active": {"row": 0, "col": 0}, "anchor": {"row": 0, "col": 0}, "ranges": [{"start": {"row": 0, "col": 0}, "end": {"row": 0, "col": 0}}]}},
        "ctx": {"author": "Ann", "timeMs": 1800000000000u64}, "rev": 1, "user": "Ann"}).to_string();
    joiner.on_message(&op, &mut j);
    assert_eq!(value(&j, "A1"), Value::Text("hi".into()));
    assert_eq!(joiner.rev(), Some(1));
}

#[test]
fn a_viewer_cant_edit_and_offline_too_long_is_read_only() {
    let (mut link, mut s) = first();
    link.on_message(&json!({"t": "Connection", "online": false}).to_string(), &mut s);
    link.tick(&mut s, 1000.0);
    link.tick(&mut s, 1000.0 + gridcraft_collab::link::OFFLINE_LIMIT_MS);
    assert!(link.offline_locked());
    assert!(s.execute("cell.set", json!({"cell": "A1", "input": "x"})).is_err());
    link.on_message(&json!({"t": "Connection", "online": true}).to_string(), &mut s);
    link.tick(&mut s, 1000.0 + gridcraft_collab::link::OFFLINE_LIMIT_MS + 10.0);
    assert!(!link.offline_locked());
    s.execute("cell.set", json!({"cell": "A1", "input": "x"})).unwrap();
}

/// A page with no file to open (`sharing_what_is_open`): the first person's open workbook
/// becomes revision 0 straight away, so the people after them aren't left waiting.
#[test]
fn with_nothing_to_open_the_first_person_shares_what_is_open() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "kept"})).unwrap();
    let mut link = CollabLink::new(None).sharing_what_is_open();
    let welcome = json!({"t": "Welcome", "rev": 0, "snapshot": null, "ops": [], "you": "me"}).to_string();
    assert!(link.on_message(&welcome, &mut s).is_empty());
    let out = link.tick(&mut s, 0.0);
    let snapshot = out.iter().map(|m| parse(m)).find(|m| m["t"] == "Snapshot").expect("revision 0 is sent at once");
    assert_eq!(snapshot["rev"], 0);
    assert!(link.take_document_ready());
    assert_eq!(value(&s, "A1"), Value::Text("kept".into()));
    // Without it, the link waits for the page's file.
    let mut waiting = CollabLink::new(None);
    let mut t = Session::new();
    t.new_workbook();
    waiting.on_message(&welcome, &mut t);
    assert!(waiting.tick(&mut t, 0.0).is_empty());
}
