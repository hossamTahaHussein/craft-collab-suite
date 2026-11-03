//! An in-memory relay and people for presentation co-editing tests: edits travel only when a test
//! says so.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Arc;

use craft_collab_core::doc::CollabDoc;
use deckcraft_collab::{Replica, SlideOp, SlidesDoc};
use deckcraft_engine::Session;
use deckcraft_model::Presentation;
use serde_json::Value;

pub struct Peer {
    pub replica: Replica,
    pub visible: Session,
    pub inbox: VecDeque<SlideOp>,
}

impl Peer {
    pub fn join(client: &str, author: &str, relay: &Relay) -> Peer {
        let replica = Replica::join(client.into(), 0, relay.base.clone(), relay.log.clone()).unwrap();
        let mut visible = Session::new();
        visible.prefs.author = author.into();
        replica.attach(&mut visible);
        Peer { replica, visible, inbox: VecDeque::new() }
    }

    /// Run a command as this person would, and hand it to the replica.
    pub fn act(&mut self, id: &str, params: Value) {
        let _ = self.visible.execute(id, &params);
        self.replica.drain_local(&mut self.visible);
    }

    pub fn deliver(&mut self, n: usize) {
        for _ in 0..n {
            let Some(op) = self.inbox.pop_front() else { return };
            self.replica.receive(op, &mut self.visible).unwrap();
        }
    }

    pub fn doc(&self) -> &Presentation {
        SlidesDoc::doc(&self.visible)
    }

    pub fn confirmed_hash(&self) -> String {
        SlidesDoc::hash(self.replica.confirmed())
    }

    pub fn visible_hash(&self) -> String {
        SlidesDoc::hash(SlidesDoc::doc(&self.visible))
    }
}

pub struct Relay {
    pub base: Arc<Presentation>,
    pub log: Vec<SlideOp>,
}

impl Relay {
    /// A deck of two title slides.
    pub fn new() -> Relay {
        let mut s = Session::with_new();
        let _ = s.execute("slide.new", &serde_json::json!({}));
        Relay { base: s.doc().unwrap().doc.clone(), log: Vec::new() }
    }

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
