//! An in-memory relay and people for workbook co-editing tests: edits travel only when a test
//! says so.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Arc;

use craft_collab_core::doc::CollabDoc;
use gridcraft_collab::{Replica, SheetOp, SheetsDoc};
use gridcraft_core::{CellRef, Value};
use gridcraft_engine::Session;
use gridcraft_model::Workbook;
use serde_json::{Value as Json, json};

pub struct Peer {
    pub replica: Replica,
    pub visible: Session,
    pub inbox: VecDeque<SheetOp>,
}

impl Peer {
    pub fn join(client: &str, author: &str, relay: &Relay) -> Peer {
        let replica = Replica::join(client.into(), 0, relay.base.clone(), relay.log.clone()).unwrap();
        let mut visible = Session::new();
        visible.prefs.user_name = author.into();
        replica.attach(&mut visible);
        Peer { replica, visible, inbox: VecDeque::new() }
    }

    /// Run a command as this person would, and hand it to the replica.
    pub fn act(&mut self, id: &str, params: Json) {
        let _ = self.visible.execute(id, params);
        self.replica.drain_local(&mut self.visible);
    }

    /// Select cell `a1` on the active sheet.
    pub fn go(&mut self, a1: &str) {
        let _ = self.visible.execute("edit.goTo", json!({"reference": a1}));
    }

    pub fn deliver(&mut self, n: usize) {
        for _ in 0..n {
            let Some(op) = self.inbox.pop_front() else { return };
            self.replica.receive(op, &mut self.visible).unwrap();
        }
    }

    pub fn value(&self, a1: &str) -> Value {
        SheetsDoc::doc(&self.visible).active().map_or(Value::Empty, |sh| sh.value(CellRef::parse(a1).unwrap()))
    }

    pub fn confirmed_hash(&self) -> String {
        SheetsDoc::hash(&Arc::new((**self.replica.confirmed()).clone()))
    }

    pub fn visible_hash(&self) -> String {
        SheetsDoc::hash(SheetsDoc::doc(&self.visible))
    }
}

pub struct Relay {
    pub base: Arc<Workbook>,
    pub log: Vec<SheetOp>,
}

impl Relay {
    pub fn new() -> Relay {
        let mut wb = Workbook::new();
        wb.sheets.push(Arc::new(gridcraft_model::Sheet::new("Sheet2")));
        Relay { base: Arc::new(wb), log: Vec::new() }
    }

    /// Take each person's next edit (if any) in the given order, stamp it and queue it for
    /// everyone.
    pub fn collect(&mut self, peers: &mut [&mut Peer], order: &[usize]) -> usize {
        let mut n = 0;
        for &i in order {
            let p = &mut *peers[i];
            p.replica.drain_local(&mut p.visible);
            if let Some(mut op) = p.replica.next_to_send() {
                op.rev = Some(self.log.len() as u64 + 1);
                op.user = Some(op.ctx.author.clone());
                self.log.push(op.clone());
                for p in peers.iter_mut() {
                    p.inbox.push_back(op.clone());
                }
                n += 1;
            }
        }
        n
    }

    /// Send and deliver until nothing moves.
    pub fn settle(&mut self, peers: &mut [&mut Peer]) {
        let order: Vec<usize> = (0..peers.len()).collect();
        loop {
            let sent = self.collect(peers, &order);
            let mut delivered = 0;
            for p in peers.iter_mut() {
                delivered += p.inbox.len();
                p.deliver(usize::MAX);
            }
            if sent == 0 && delivered == 0 {
                return;
            }
        }
    }
}
