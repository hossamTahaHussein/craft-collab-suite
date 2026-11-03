//! Random interleavings of people editing one workbook: every replica ends with the same one,
//! the one a replay of the relay's log gives, and what each person sees is what is confirmed.

mod support;

use std::sync::Arc;

use craft_collab_core::doc::CollabDoc;
use gridcraft_collab::{Replica, SheetsDoc};
use gridcraft_core::Value;
use proptest::prelude::*;
use serde_json::json;
use support::{Peer, Relay};

#[derive(Clone, Debug)]
enum Action {
    Set(usize, &'static str, &'static str),
    InsertRows(usize, u32),
    DeleteRows(usize, u32),
    InsertCols(usize, u32),
    DeleteCols(usize, u32),
    InsertSheet(usize),
    RenameSheet(usize, &'static str),
    DeleteSheet(usize),
    SwitchSheet(usize),
    Undo(usize),
    Redo(usize),
    Send(usize),
    Deliver(usize, usize),
}

const CELLS: &[&str] = &["A1", "B2", "C3", "A4", "D2", "B6"];
const INPUTS: &[&str] = &["1", "two", "=A1+1", "=SUM(A1:D6)", "=B2&\"!\"", "3.5", "=NOW()>0", "=RAND()<2"];
const NAMES: &[&str] = &["Data", "Notes", "Q3"];

fn action(peers: usize) -> impl Strategy<Value = Action> {
    let p = 0..peers;
    prop_oneof![
        6 => (p.clone(), prop::sample::select(CELLS), prop::sample::select(INPUTS)).prop_map(|(i, c, v)| Action::Set(i, c, v)),
        1 => (p.clone(), 1u32..6).prop_map(|(i, r)| Action::InsertRows(i, r)),
        1 => (p.clone(), 1u32..6).prop_map(|(i, r)| Action::DeleteRows(i, r)),
        1 => (p.clone(), 1u32..4).prop_map(|(i, c)| Action::InsertCols(i, c)),
        1 => (p.clone(), 1u32..4).prop_map(|(i, c)| Action::DeleteCols(i, c)),
        1 => p.clone().prop_map(Action::InsertSheet),
        1 => (p.clone(), prop::sample::select(NAMES)).prop_map(|(i, n)| Action::RenameSheet(i, n)),
        1 => p.clone().prop_map(Action::DeleteSheet),
        1 => p.clone().prop_map(Action::SwitchSheet),
        2 => p.clone().prop_map(Action::Undo),
        1 => p.clone().prop_map(Action::Redo),
        3 => p.clone().prop_map(Action::Send),
        3 => (p, 1usize..4).prop_map(|(i, k)| Action::Deliver(i, k)),
    ]
}

fn col(c: u32) -> String {
    char::from(b'A' + c as u8).to_string()
}

fn play(peers: &mut [Peer], relay: &mut Relay, a: &Action) {
    match *a {
        Action::Set(i, cell, input) => peers[i].act("cell.set", json!({"cell": cell, "input": input})),
        Action::InsertRows(i, r) => peers[i].act("home.insertRows", json!({"rows": format!("{r}:{r}")})),
        Action::DeleteRows(i, r) => peers[i].act("home.deleteRows", json!({"rows": format!("{r}:{r}")})),
        Action::InsertCols(i, c) => peers[i].act("home.insertColumns", json!({"cols": format!("{0}:{0}", col(c))})),
        Action::DeleteCols(i, c) => peers[i].act("home.deleteColumns", json!({"cols": format!("{0}:{0}", col(c))})),
        Action::InsertSheet(i) => peers[i].act("home.insertSheet", json!({})),
        Action::RenameSheet(i, name) => peers[i].act("sheet.rename", json!({"name": name})),
        Action::DeleteSheet(i) => peers[i].act("home.deleteSheet", json!({})),
        Action::SwitchSheet(i) => {
            let n = SheetsDoc::doc(&peers[i].visible).sheets.len();
            let at = SheetsDoc::doc(&peers[i].visible).active_sheet();
            let _ = peers[i].visible.execute("sheet.activate", json!({"sheet": (at + 1) % n.max(1)}));
        }
        Action::Undo(i) => peers[i].act("edit.undo", json!({})),
        Action::Redo(i) => peers[i].act("edit.redo", json!({})),
        Action::Send(i) => {
            let mut refs: Vec<&mut Peer> = peers.iter_mut().collect();
            relay.collect(&mut refs, &[i]);
        }
        Action::Deliver(i, k) => peers[i].deliver(k),
    }
}

fn check(peers: &mut [Peer], relay: &mut Relay) -> Result<(), TestCaseError> {
    let mut refs: Vec<&mut Peer> = peers.iter_mut().collect();
    relay.settle(&mut refs);
    let fresh = Replica::join("fresh".into(), 0, relay.base.clone(), relay.log.clone()).unwrap();
    let want = SheetsDoc::hash(&Arc::new((**fresh.confirmed()).clone()));
    for (i, p) in peers.iter().enumerate() {
        prop_assert_eq!(p.replica.pending_len(), 0, "person {} has unconfirmed edits", i);
        prop_assert_eq!(p.confirmed_hash(), want.clone(), "person {}'s confirmed workbook differs from a replay of the log", i);
        prop_assert_eq!(p.visible_hash(), want.clone(), "person {} sees something else", i);
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(48), failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn random_interleavings_converge(actions in prop::collection::vec(action(3), 1..40)) {
        let mut relay = Relay::new();
        let mut peers: Vec<Peer> = ["a", "b", "c"].iter().map(|c| Peer::join(c, &c.to_uppercase(), &relay)).collect();
        for a in &actions {
            play(&mut peers, &mut relay, a);
        }
        check(&mut peers, &mut relay)?;
    }
}

/// Someone inserts rows above the cell another person is about to enter: the entry lands in the
/// cell they chose, which moved down.
#[test]
fn an_entry_follows_its_cell_down_past_inserted_rows() {
    let mut relay = Relay::new();
    let mut a = Peer::join("a", "Ann", &relay);
    let mut b = Peer::join("b", "Bob", &relay);
    b.act("home.insertRows", json!({"rows": "1:2"}));
    relay.collect(&mut [&mut a, &mut b], &[1]);
    // Ann enters B3 before Bob's insert reaches her.
    a.act("cell.set", json!({"cell": "B3", "input": "mine"}));
    relay.settle(&mut [&mut a, &mut b]);
    assert_eq!(a.value("B5"), Value::Text("mine".into()));
    assert_eq!(b.value("B5"), Value::Text("mine".into()));
    assert_eq!(b.value("B3"), Value::Empty);
}
