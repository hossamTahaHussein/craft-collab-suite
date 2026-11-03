//! The generic replica on [`LineDoc`]: the behaviour every app's adapter relies on.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;

use serde_json::json;

use crate::line_doc::{LineDoc, Session, Splice};
use crate::proto::{HistEntry, Inbound, MAX_OP_BYTES, Op, OpKind, Outbound, ProtoError, decode_inbound};
use crate::replica::{Applied, CollabError, Replica};

fn lines(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

struct Peer {
    replica: Replica<LineDoc>,
    visible: Session,
    inbox: VecDeque<Op<LineDoc>>,
}

struct Relay {
    base: Vec<String>,
    log: Vec<Op<LineDoc>>,
}

impl Relay {
    fn new(base: &[&str]) -> Relay {
        Relay { base: lines(base), log: Vec::new() }
    }

    fn join(&self, client: &str) -> Peer {
        let replica = Replica::join(client.into(), 0, self.base.clone(), self.log.clone()).unwrap();
        let mut visible = Session::new(Vec::new());
        visible.author = client.to_uppercase();
        replica.attach(&mut visible);
        Peer { replica, visible, inbox: VecDeque::new() }
    }

    /// Take `peers[i]`'s next op, stamp it and queue it for everyone.
    fn send(&mut self, peers: &mut [Peer], i: usize) -> bool {
        peers[i].replica.drain_local(&mut peers[i].visible);
        let Some(mut op) = peers[i].replica.next_to_send() else { return false };
        op.rev = Some(self.log.len() as u64 + 1);
        self.log.push(op.clone());
        for p in peers.iter_mut() {
            p.inbox.push_back(op.clone());
        }
        true
    }

    fn settle(&mut self, peers: &mut [Peer]) {
        loop {
            let mut moved = false;
            for i in 0..peers.len() {
                moved |= self.send(peers, i);
            }
            for p in peers.iter_mut() {
                while let Some(op) = p.inbox.pop_front() {
                    p.replica.receive(op, &mut p.visible).unwrap();
                    moved = true;
                }
            }
            if !moved {
                return;
            }
        }
    }
}

fn act(p: &mut Peer, cmd: &str, text: &str) {
    p.visible.run(cmd, json!({ "text": text }));
}

#[test]
fn concurrent_edits_converge_and_own_edits_show_at_once() {
    let mut relay = Relay::new(&["a", "b", "c"]);
    let mut peers = vec![relay.join("x"), relay.join("y")];
    peers[0].visible.sel = 1;
    act(&mut peers[0], "line.insert", "x1");
    peers[1].visible.sel = 3;
    act(&mut peers[1], "line.insert", "y1");
    assert_eq!(peers[0].visible.lines, lines(&["a", "x1", "b", "c"]), "shows at once");
    relay.settle(&mut peers);
    for p in &peers {
        assert_eq!(p.visible.lines, lines(&["a", "x1", "b", "c", "y1"]));
        assert_eq!(p.replica.confirmed(), &p.visible.lines);
        assert_eq!(p.replica.pending_len(), 0);
    }
}

#[test]
fn a_pending_edit_is_rebased_past_someone_elses() {
    let mut relay = Relay::new(&["a", "b"]);
    let mut peers = vec![relay.join("x"), relay.join("y")];
    // y inserts at the top and it reaches the relay first; x's edit to "b" waits.
    act(&mut peers[1], "line.insert", "top");
    assert!(relay.send(&mut peers, 1));
    peers[0].visible.sel = 1;
    act(&mut peers[0], "line.set", "B");
    let x = &mut peers[0];
    x.replica.drain_local(&mut x.visible);
    let op = x.inbox.pop_front().unwrap();
    assert_eq!(x.replica.receive(op, &mut x.visible).unwrap(), Applied::Remote { dropped_pending: 0 });
    assert_eq!(peers[0].visible.lines, lines(&["top", "a", "B"]), "x's edit moved down with its line");
    relay.settle(&mut peers);
    assert_eq!(peers[1].visible.lines, lines(&["top", "a", "B"]));
}

#[test]
fn echo_duplicate_and_gap() {
    let relay = Relay::new(&["a"]);
    let mut p = relay.join("x");
    act(&mut p, "line.insert", "n");
    p.replica.drain_local(&mut p.visible);
    let mut op = p.replica.next_to_send().unwrap();
    assert!(p.replica.next_to_send().is_none(), "one edit in flight");
    op.rev = Some(1);
    assert_eq!(p.replica.receive(op.clone(), &mut p.visible).unwrap(), Applied::Echo);
    assert_eq!(p.replica.receive(op.clone(), &mut p.visible).unwrap(), Applied::Duplicate);
    op.rev = Some(5);
    assert_eq!(p.replica.receive(op, &mut p.visible), Err(CollabError::Gap { have: 1, got: 5 }));
}

#[test]
fn undo_takes_back_only_ones_own_edit_for_everyone() {
    let mut relay = Relay::new(&["a"]);
    let mut peers = vec![relay.join("x"), relay.join("y")];
    act(&mut peers[0], "line.insert", "mine");
    relay.settle(&mut peers);
    peers[1].visible.sel = 2;
    act(&mut peers[1], "line.insert", "theirs");
    relay.settle(&mut peers);
    assert!(peers[0].replica.can_undo());
    peers[0].visible.outbox.push(crate::doc::Local { cmd: "edit.undo".into(), params: json!({}), sel: 0, ctx: Default::default() });
    relay.settle(&mut peers);
    for p in &peers {
        assert_eq!(p.visible.lines, lines(&["a", "theirs"]));
    }
    assert!(peers[0].replica.can_redo());
}

#[test]
fn a_refused_edit_leaves_the_visible_document() {
    let relay = Relay::new(&["a"]);
    let mut p = relay.join("x");
    act(&mut p, "line.insert", "n");
    p.replica.drain_local(&mut p.visible);
    let op = p.replica.next_to_send().unwrap();
    p.replica.refused(op.seq, &mut p.visible);
    assert_eq!(p.visible.lines, lines(&["a"]));
    assert_eq!(p.replica.pending_len(), 0);
}

#[test]
fn history_places_an_edit_based_on_an_earlier_revision() {
    let relay = Relay::new(&["a", "b"]);
    let history = vec![HistEntry::<LineDoc> { rev: 1, change: crate::line_doc::Wire { splice: Some(Splice { at: 0, old: 0, new: 1 }) } }];
    let r = Replica::<LineDoc>::join_with_history("x".into(), 1, lines(&["top", "a", "b"]), history, Vec::new()).unwrap();
    assert_eq!(r.rebase_anchor(&1, 0), Some(2), "line 1 at revision 0 is line 2 now");
    assert_eq!(r.rebase_anchor(&1, 2), None, "newer than the replica");
    let _ = relay;
}

#[test]
fn snapshot_carries_history_and_hash_is_due_every_50() {
    let mut relay = Relay::new(&["a"]);
    let mut peers = vec![relay.join("x")];
    let mut due = Vec::new();
    for i in 0..60 {
        act(&mut peers[0], "line.insert", &i.to_string());
        relay.settle(&mut peers);
        if let Some(Outbound::Hash { rev, .. }) = peers[0].replica.hash_due() {
            due.push(rev);
        }
    }
    assert_eq!(due, [50]);
    let Ok(Outbound::Snapshot { rev, history, .. }) = peers[0].replica.snapshot() else { panic!("snapshot") };
    assert_eq!(rev, 60);
    assert_eq!(history.len(), 60);
}

#[test]
fn messages_keep_their_wire_shape() {
    let op = Op::<LineDoc> {
        client: "c".into(),
        seq: 1,
        base: 0,
        kind: OpKind::Cmd,
        cmd: "line.set".into(),
        params: json!({"text": "t"}),
        sel: 2,
        ctx: Default::default(),
        rev: None,
        user: None,
    };
    let s = serde_json::to_string(&Outbound::Op(Box::new(op.clone()))).unwrap();
    assert_eq!(
        s,
        r#"{"t":"Op","client":"c","seq":1,"base":0,"kind":"cmd","cmd":"line.set","params":{"text":"t"},"sel":2,"ctx":{"author":"","time":0,"api":false}}"#
    );
    let e = HistEntry::<LineDoc> { rev: 3, change: crate::line_doc::Wire { splice: None } };
    assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"rev":3,"splice":null}"#, "the app's change is flattened into the entry");
    let Inbound::Op(back) = decode_inbound::<LineDoc>(&s).unwrap() else { panic!("op") };
    assert_eq!(*back, op);
    let big = format!(r#"{{"t":"Leave","client":"{}"}}"#, "x".repeat(MAX_OP_BYTES));
    assert!(matches!(decode_inbound::<LineDoc>(&big), Err(ProtoError::TooLarge { .. })));
}
