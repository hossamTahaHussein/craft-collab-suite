//! Co-editing mode (`crate::collab`): the person's edits are recorded once, with the place and
//! context they ran in; replaying one under that context gives the same workbook anywhere.

use gridcraft_core::{CellRef, Value};
use serde_json::json;

use crate::Session;
use crate::collab::RunCtx;

fn s() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.set_collab(true);
    s
}

fn v(s: &Session, a: &str) -> Value {
    s.doc().unwrap().wb.active().unwrap().value(CellRef::parse(a).unwrap())
}

#[test]
fn the_outbox_holds_each_edit_that_changed_the_workbook_once() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "B2", "input": "5"})).unwrap();
    s.execute("view.zoom", json!({"value": 150})).ok();
    s.execute("cell.set", json!({"cell": "B2", "input": "5"})).unwrap();
    let out = s.take_outbox();
    assert_eq!(out.len(), 1, "a query, a view change and an edit that changed nothing aren't sent: {out:?}");
    assert_eq!(out[0].id, "cell.set");
    let place = out[0].place.as_ref().unwrap();
    assert_eq!(place.sheet, "Sheet1");
    assert!(s.take_outbox().is_empty(), "taken");
}

#[test]
fn replaying_under_the_edits_context_fixes_now_and_rand() {
    let ctx = RunCtx { author: "Ann".into(), time_ms: 1_800_000_000_000, ..RunCtx::default() };
    let run = || {
        let mut s = s();
        s.set_recording(false);
        s.execute_with(&ctx, "cell.set", json!({"cell": "A1", "input": "=NOW()"})).unwrap();
        s.execute_with(&ctx, "cell.set", json!({"cell": "A2", "input": "=RAND()"})).unwrap();
        (v(&s, "A1"), v(&s, "A2"))
    };
    let (now, r) = run();
    assert_eq!(now, Value::Number(gridcraft_core::date::serial_from_unix_ms(1_800_000_000_000.0)));
    assert_eq!(run(), (now, r));
}

#[test]
fn what_co_editing_cant_do_is_refused_with_a_reason() {
    let mut s = s();
    for (cmd, p) in [("file.new", json!({})), ("file.open", json!({"path": "/tmp/x.xlsx"})), ("macro.run", json!({"name": "M"}))] {
        let e = s.execute(cmd, p).unwrap_err().to_string();
        assert!(e.contains("co-editing"), "{cmd}: {e}");
    }
    assert!(s.take_outbox().is_empty());
}

#[test]
fn undo_and_redo_go_to_the_co_editing_layer() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(1.0), "not undone on this session's own history");
    let ids: Vec<String> = s.take_outbox().into_iter().map(|r| r.id).collect();
    assert_eq!(ids, ["cell.set", "edit.undo"]);
}

#[test]
fn a_replayed_edit_is_not_recorded_again() {
    let mut s = s();
    s.set_recording(false);
    s.execute_with(&RunCtx::default(), "cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    assert!(s.take_outbox().is_empty());
    assert_eq!(v(&s, "A1"), Value::Number(1.0));
}

/// Another person's edit is replayed on every replica: one that runs code or reaches this
/// computer's files must not run here, whatever the relay passed on.
#[test]
fn a_replayed_edit_cant_run_code_or_read_this_computers_files() {
    let mut s = s();
    s.set_recording(false);
    let ctx = RunCtx::default();
    for (cmd, p) in
        [("macro.run", json!({"name": "M"})), ("python.run", json!({})), ("file.open", json!({"path": "/etc/hostname"})), ("edit.undo", json!({}))]
    {
        assert!(s.execute_with(&ctx, cmd, p).is_err(), "{cmd} replayed");
    }
    let file = std::env::temp_dir().join(format!("gridcraft-collab-replay-{}.csv", std::process::id()));
    std::fs::write(&file, "secret,1\n").unwrap();
    let r = s.execute_with(&ctx, "data.fromTextCsv", json!({"path": file.to_str().unwrap()}));
    let _ = std::fs::remove_file(&file);
    assert!(r.is_err(), "a replayed import from a local path: {r:?}");
    assert_eq!(v(&s, "A1"), Value::Empty);
}

#[test]
fn a_structural_edit_is_sent_even_where_it_changed_no_content() {
    let mut s = s();
    s.execute("home.insertRows", json!({"rows": "1:2"})).unwrap();
    assert_eq!(s.take_outbox().len(), 1, "rows inserted into an empty sheet move other people's positions");
}

/// Shared content is never this computer's own: whatever the workbook it replaces was trusted
/// with, it is trusted as anything read from elsewhere is.
#[test]
fn a_shared_workbook_is_not_trusted_as_a_local_one() {
    let mut s = s();
    assert_eq!(s.doc().unwrap().trust, crate::Trust::Local);
    s.set_collab_workbook(std::sync::Arc::new(gridcraft_model::Workbook::new()));
    let d = s.doc().unwrap();
    assert_eq!(d.trust, crate::Trust::FromFile);
    assert!(!d.scripts_trusted);
}

/// A viewer changes nothing; a commenter only comments; read only (offline too long) is a
/// viewer too, until it is lifted.
#[test]
fn roles_and_read_only_limit_what_the_person_may_change() {
    use crate::collab::CollabRole;
    let mut s = s();
    s.set_collab_role(CollabRole::View);
    assert!(s.execute("cell.set", json!({"cell": "A1", "input": "x"})).is_err());
    assert_eq!(v(&s, "A1"), Value::Empty, "nothing changed");
    assert!(s.execute("edit.undo", json!({})).is_err());
    s.execute("edit.goTo", json!({"reference": "C3"})).unwrap();
    s.set_collab_role(CollabRole::Comment);
    assert!(s.execute("cell.set", json!({"cell": "A1", "input": "x"})).is_err());
    s.execute("review.newNote", json!({"cell": "B2", "text": "check"})).unwrap();
    s.set_collab_role(CollabRole::Edit);
    s.set_read_only(true, true);
    assert!(s.execute("cell.set", json!({"cell": "A1", "input": "x"})).is_err());
    s.set_read_only(false, false);
    s.execute("cell.set", json!({"cell": "A1", "input": "x"})).unwrap();
    let ids: Vec<String> = s.take_outbox().into_iter().map(|r| r.id).collect();
    assert_eq!(ids, ["review.newNote", "cell.set"]);
}

/// The relay's HTTP API runs a command through this person's editor as its own author: the
/// edit carries that name and its initials and is marked as the API's (not this person's to
/// undo); the person's own name is back afterwards.
#[test]
fn an_api_edit_is_recorded_as_the_apis() {
    let mut s = s();
    s.prefs.user_name = "Ann Lee".into();
    let r = s.as_api_author("Report Bot", |s| s.execute("cell.set", json!({"cell": "A1", "input": "7"})));
    assert!(r.is_ok());
    s.execute("cell.set", json!({"cell": "A2", "input": "8"})).unwrap();
    let out = s.take_outbox();
    assert_eq!(out.len(), 2);
    assert_eq!((out[0].ctx.author.as_str(), out[0].ctx.initials.as_str(), out[0].ctx.api), ("Report Bot", "RB", true));
    assert_eq!((out[1].ctx.author.as_str(), out[1].ctx.api), ("Ann Lee", false), "the person's own edits are theirs again");
}
