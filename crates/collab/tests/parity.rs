//! A recorded log of 2,000 edits by five people (`tests/fixtures/parity-2k.json`). Making the
//! same edits again must give the same log, byte for byte, and replaying the log must give the
//! same documents: a change to the co-editing core that alters what Docs does fails here.
//!
//! `cargo test -p wordcraft-collab --test parity -- --ignored record_parity_log` writes the
//! fixture again (only when a change of behaviour is intended).

mod support;

use serde_json::{Value, json};
use support::{Peer, Relay, doc_with};
use wordcraft_collab::hash::doc_hash;
use wordcraft_collab::proto::Op;
use wordcraft_collab::replica::Replica;

const SEED: u64 = 0x5EED_C011AB;
const PEOPLE: usize = 5;
const EDITS: usize = 2_000;
const BASE: &str = "The quick brown fox jumps over the lazy dog.";
/// A document fingerprint every this many revisions (and at the end).
const EVERY: usize = 250;
const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/parity-2k.json");
const TEXTS: &[&str] = &["a", "bc", "Hello ", "مرحبا", "👋", "x y", "1.", "e\u{301}"];

/// xorshift64*: the log must not depend on any library's random generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// One person does one thing, chosen with the weights of `tests/convergence.rs`.
fn step(rng: &mut Rng, peers: &mut [Peer], relay: &mut Relay) {
    let i = rng.below(PEOPLE);
    let text = TEXTS[rng.below(TEXTS.len())];
    match rng.below(26) {
        0..=3 => peers[i].type_text(text),
        4..=5 => {
            let _ = peers[i].visible.run("text.insert", &json!({ "text": text }));
        }
        6 => peers[i].act("text.newParagraph", json!({})),
        7 => peers[i].act("text.backspace", json!({})),
        8 => peers[i].act("text.delete", json!({})),
        9..=10 => {
            let id = if rng.below(2) == 0 { "caret.left" } else { "caret.right" };
            for _ in 0..rng.below(6) + 1 {
                peers[i].act(id, json!({}));
            }
        }
        11 => {
            peers[i].act("select.word", json!({}));
            peers[i].act("format.bold", json!({}));
        }
        12 => {
            peers[i].act("select.word", json!({}));
            peers[i].type_text(text);
        }
        13 => {
            peers[i].act("select.word", json!({}));
            peers[i].act("review.newComment", json!({"text": "note"}));
        }
        14 => peers[i].act("insert.table", json!({"rows": 2, "cols": 2})),
        15..=16 => peers[i].act("edit.undo", json!({})),
        17 => peers[i].act("edit.redo", json!({})),
        // A network round trip: everyone's next edit goes to the relay (starting with this
        // person), then everyone receives everything queued.
        18..=21 => {
            let order: Vec<usize> = (0..PEOPLE).map(|k| (i + k) % PEOPLE).collect();
            let mut refs: Vec<&mut Peer> = peers.iter_mut().collect();
            relay.collect(&mut refs, &order);
            for p in peers.iter_mut() {
                p.deliver(usize::MAX);
            }
        }
        _ => peers[i].deliver(rng.below(16) + 1),
    }
}

fn hash_after(log: &[Op], n: usize) -> String {
    let replica = Replica::join("parity".into(), 0, doc_with(BASE), log[..n].to_vec()).unwrap();
    doc_hash(replica.confirmed())
}

/// Five people edit until the relay has sequenced `EDITS` edits, then everything settles.
fn record() -> Value {
    let mut rng = Rng(SEED);
    let mut relay = Relay::new(doc_with(BASE));
    let mut peers: Vec<Peer> = (0..PEOPLE).map(|i| Peer::join(&format!("c{i}"), &format!("Person {i}"), &relay)).collect();
    // A clock of its own: edits carry their time, and typing merges by it.
    let mut clock = 1_800_000_000u64;
    while relay.log.len() < EDITS {
        clock += rng.below(3) as u64;
        wordcraft_engine::run_ctx::with_clock(clock, || step(&mut rng, &mut peers, &mut relay));
    }
    let mut refs: Vec<&mut Peer> = peers.iter_mut().collect();
    relay.settle(&mut refs);
    let log = relay.log.clone();
    let mut marks: Vec<usize> = (EVERY..=log.len()).step_by(EVERY).collect();
    if marks.last() != Some(&log.len()) {
        marks.push(log.len());
    }
    let hashes: Vec<Value> = marks.iter().map(|&n| json!({"rev": n, "hash": hash_after(&log, n)})).collect();
    let wire: Vec<String> = log.iter().map(|op| serde_json::to_string(op).unwrap()).collect();
    json!({"seed": SEED, "people": PEOPLE, "base": BASE, "wire": wire, "hashes": hashes})
}

fn fixture() -> Value {
    let text = std::fs::read_to_string(FIXTURE).expect("tests/fixtures/parity-2k.json (record it with --ignored record_parity_log)");
    serde_json::from_str(&text).unwrap()
}

#[test]
#[ignore]
fn record_parity_log() {
    let log = record();
    std::fs::create_dir_all(std::path::Path::new(FIXTURE).parent().unwrap()).unwrap();
    std::fs::write(FIXTURE, serde_json::to_string(&log).unwrap() + "\n").unwrap();
}

/// The same edits, made again, give the same log byte for byte.
#[test]
fn the_same_edits_give_the_same_log() {
    let want = fixture();
    let got = record();
    assert_eq!(got["wire"], want["wire"], "the relay's log differs from the recorded one");
    assert_eq!(got["hashes"], want["hashes"]);
}

/// Every recorded edit reads and writes back as the same JSON, and replaying the log gives the
/// recorded documents.
#[test]
fn replay_matches_parity_log() {
    let want = fixture();
    let wire: Vec<String> = want["wire"].as_array().unwrap().iter().map(|w| w.as_str().unwrap().to_string()).collect();
    let log: Vec<Op> = wire.iter().map(|w| serde_json::from_str(w).unwrap()).collect();
    for (w, op) in wire.iter().zip(&log) {
        assert_eq!(&serde_json::to_string(op).unwrap(), w);
    }
    for h in want["hashes"].as_array().unwrap() {
        let n = h["rev"].as_u64().unwrap() as usize;
        assert_eq!(hash_after(&log, n), h["hash"].as_str().unwrap(), "after {n} edits");
    }
}
