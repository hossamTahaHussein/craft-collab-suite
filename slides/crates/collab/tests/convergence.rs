//! Random interleavings of people editing one presentation: every replica ends with the same
//! deck, the one a replay of the relay's log gives, and what each person sees is what is
//! confirmed.

mod support;

use craft_collab_core::doc::CollabDoc;
use deckcraft_collab::{Replica, SlidesDoc, change::linear};
use proptest::prelude::*;
use serde_json::json;
use support::{Peer, Relay};

#[derive(Clone, Debug)]
enum Action {
    EditTitle(usize),
    Type(usize, &'static str),
    Backspace(usize),
    Caret(usize, bool),
    NewShape(usize),
    MoveFirstShape(usize, f64),
    DeleteFirstShape(usize),
    NewSlide(usize),
    MoveSlide(usize, usize, usize),
    DeleteSlide(usize, usize),
    NextSlide(usize),
    Comment(usize),
    Undo(usize),
    Redo(usize),
    Send(usize),
    Deliver(usize, usize),
}

const TEXTS: &[&str] = &["a", "bc", "Hello ", "مرحبا", "x y", "1."];

fn action(peers: usize) -> impl Strategy<Value = Action> {
    let p = 0..peers;
    prop_oneof![
        2 => p.clone().prop_map(Action::EditTitle),
        5 => (p.clone(), prop::sample::select(TEXTS)).prop_map(|(i, t)| Action::Type(i, t)),
        1 => p.clone().prop_map(Action::Backspace),
        1 => (p.clone(), any::<bool>()).prop_map(|(i, b)| Action::Caret(i, b)),
        1 => p.clone().prop_map(Action::NewShape),
        1 => (p.clone(), 10.0f64..200.0).prop_map(|(i, x)| Action::MoveFirstShape(i, x)),
        1 => p.clone().prop_map(Action::DeleteFirstShape),
        1 => p.clone().prop_map(Action::NewSlide),
        1 => (p.clone(), 0usize..3, 0usize..3).prop_map(|(i, a, b)| Action::MoveSlide(i, a, b)),
        1 => (p.clone(), 0usize..3).prop_map(|(i, a)| Action::DeleteSlide(i, a)),
        1 => p.clone().prop_map(Action::NextSlide),
        1 => p.clone().prop_map(Action::Comment),
        2 => p.clone().prop_map(Action::Undo),
        1 => p.clone().prop_map(Action::Redo),
        3 => p.clone().prop_map(Action::Send),
        3 => (p, 1usize..4).prop_map(|(i, k)| Action::Deliver(i, k)),
    ]
}

fn first_shape(p: &Peer) -> Option<u32> {
    let d = p.visible.active()?;
    d.doc.slides.get(d.selection.slide)?.shapes.first().map(|s| s.id.0)
}

fn play(peers: &mut [Peer], relay: &mut Relay, a: &Action) {
    match *a {
        Action::EditTitle(i) => {
            if let Some(id) = first_shape(&peers[i]) {
                peers[i].act("text.edit", json!({"id": id}));
            }
        }
        Action::Type(i, t) => peers[i].act("text.insert", json!({"text": t})),
        Action::Backspace(i) => peers[i].act("text.delete", json!({"dir": "backward"})),
        Action::Caret(i, end) => peers[i].act("text.move", json!({"to": if end { "end" } else { "start" }})),
        Action::NewShape(i) => peers[i].act("shape.insert", json!({"preset": "rect", "rect": [80.0, 80.0, 300.0, 160.0], "text": "box"})),
        Action::MoveFirstShape(i, x) => {
            if let Some(id) = first_shape(&peers[i]) {
                peers[i].act("shape.setBounds", json!({"id": id, "rect": [x, 40.0, 200.0, 80.0]}));
            }
        }
        Action::DeleteFirstShape(i) => {
            if let Some(id) = first_shape(&peers[i]) {
                peers[i].act("edit.delete", json!({"ids": [id]}));
            }
        }
        Action::NewSlide(i) => peers[i].act("slide.new", json!({})),
        Action::MoveSlide(i, from, to) => peers[i].act("slide.move", json!({"from": from, "to": to})),
        Action::DeleteSlide(i, at) => peers[i].act("slide.delete", json!({"index": at})),
        Action::NextSlide(i) => {
            let n = peers[i].doc().slides.len().max(1);
            let at = peers[i].visible.active().map_or(0, |d| d.selection.slide);
            let _ = peers[i].visible.execute("slide.select", &json!({"index": (at + 1) % n}));
        }
        Action::Comment(i) => peers[i].act("comment.add", json!({"text": "note"})),
        Action::Undo(i) => peers[i].act("edit.undo", json!({})),
        Action::Redo(i) => peers[i].act("edit.redo", json!({})),
        Action::Send(i) => {
            let mut refs: Vec<&mut Peer> = peers.iter_mut().collect();
            relay.collect(&mut refs, &[i]);
        }
        Action::Deliver(i, k) => peers[i].deliver(k),
    }
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
        let mut refs: Vec<&mut Peer> = peers.iter_mut().collect();
        relay.settle(&mut refs);
        let fresh = Replica::join("fresh".into(), 0, relay.base.clone(), relay.log.clone()).unwrap();
        let want = SlidesDoc::hash(fresh.confirmed());
        for (i, p) in peers.iter().enumerate() {
            prop_assert_eq!(p.replica.pending_len(), 0, "person {} has unconfirmed edits", i);
            prop_assert_eq!(p.confirmed_hash(), want.clone(), "person {}'s confirmed deck differs from a replay of the log", i);
            prop_assert_eq!(p.visible_hash(), want.clone(), "person {} sees something else", i);
        }
    }
}

/// Two people type in the same title: each one's text stays where they typed it.
#[test]
fn typing_follows_another_persons_typing_in_the_same_shape() {
    let mut relay = Relay::new();
    let mut a = Peer::join("a", "Ann", &relay);
    let mut b = Peer::join("b", "Bob", &relay);
    let title = first_shape(&a).unwrap();
    a.act("text.edit", json!({"id": title}));
    a.act("text.insert", json!({"text": "Hello world"}));
    relay.settle(&mut [&mut a, &mut b]);
    b.act("text.edit", json!({"id": title}));
    b.act("text.move", json!({"to": "end"}));
    a.act("text.move", json!({"to": "start"}));
    a.act("text.insert", json!({"text": "Big "}));
    b.act("text.insert", json!({"text": "!"}));
    relay.settle(&mut [&mut a, &mut b]);
    let text = |p: &Peer| linear(p.doc().slides[0].shapes[0].text.as_ref().unwrap());
    assert_eq!(text(&a), "Big Hello world!");
    assert_eq!(text(&b), "Big Hello world!");
}
