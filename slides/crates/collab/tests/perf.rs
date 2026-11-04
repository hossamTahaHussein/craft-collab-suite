//! Co-editing speed (suite spec row 45): someone else's edit shows in a 100-slide presentation in
//! under 4 ms. Timed in release builds (`cargo test --release -p deckcraft-collab --test perf`); a
//! debug build only checks it runs.

mod support;

use std::time::Instant;

use deckcraft_engine::Session;
use serde_json::json;
use support::{Peer, Relay};

/// 100 slides, each with a title, a body and a text box.
fn big() -> Relay {
    let mut s = Session::with_new();
    s.execute("insert.textBox", &json!({"rect": [100, 300, 400, 80], "text": "A note on this slide"})).unwrap();
    for _ in 0..99 {
        s.execute("slide.duplicate", &json!({})).unwrap();
    }
    let mut relay = Relay::new();
    relay.base = s.doc().unwrap().doc.clone();
    relay
}

#[test]
fn someone_elses_edit_shows_in_a_big_presentation_quickly() {
    let mut relay = big();
    assert_eq!(relay.base.slides.len(), 100);
    let mut ann = Peer::join("a", "Ann", &relay);
    let mut bob = Peer::join("b", "Bob", &relay);
    let id = relay.base.slides[50].shapes.last().unwrap().id.0;
    ann.act("slide.go", json!({"index": 50}));
    let mut times = Vec::new();
    for k in 0..20 {
        ann.act("shape.move", json!({"ids": [id], "dx": 1 + k % 3}));
        relay.collect(&mut [&mut ann, &mut bob], &[0]);
        ann.deliver(usize::MAX);
        let t = Instant::now();
        bob.deliver(usize::MAX);
        times.push(t.elapsed());
    }
    assert_eq!(ann.visible_hash(), bob.visible_hash());
    times.sort();
    let median = times[times.len() / 2];
    eprintln!("a remote shape move in a 100-slide presentation: median {median:?}, worst {:?}", times.last().unwrap());
    if !cfg!(debug_assertions) {
        assert!(median.as_secs_f64() < 0.004, "median {median:?}");
    }
}

/// Twenty people moving shapes at once: every edit reaches everyone quickly, and all twenty
/// presentations end the same.
#[test]
fn twenty_people_editing_at_once_stay_quick_and_together() {
    let relay0 = big();
    let mut relay = Relay { base: relay0.base.clone(), log: Vec::new() };
    let mut people: Vec<Peer> = (0..20).map(|i| Peer::join(&format!("p{i}"), &format!("Person {i}"), &relay)).collect();
    let mut times = Vec::new();
    for round in 0..5 {
        for (i, p) in people.iter_mut().enumerate() {
            let id = relay.base.slides[i].shapes.last().unwrap().id.0;
            p.act("slide.go", json!({"index": i}));
            p.act("shape.move", json!({"ids": [id], "dy": round + 1}));
        }
        let order: Vec<usize> = (0..people.len()).collect();
        let mut refs: Vec<&mut Peer> = people.iter_mut().collect();
        relay.collect(&mut refs, &order);
        for p in people.iter_mut() {
            while !p.inbox.is_empty() {
                let t = Instant::now();
                p.deliver(1);
                times.push(t.elapsed());
            }
        }
    }
    let h = people[0].visible_hash();
    assert!(people.iter().all(|p| p.visible_hash() == h), "everyone ends with the same presentation");
    times.sort();
    let median = times[times.len() / 2];
    eprintln!("20 people editing, a delivered edit: median {median:?}, worst {:?}", times.last().unwrap());
    if !cfg!(debug_assertions) {
        assert!(median.as_secs_f64() < 0.004, "median {median:?}");
    }
}
