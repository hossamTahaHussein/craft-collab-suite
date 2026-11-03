//! A recorded log of 2,000 edits by five people (`tests/fixtures/parity-2k.json.z`, zlib-compressed JSON: restores carry whole documents). Making the
//! same edits again must give the same log, byte for byte, and replaying the log must give the
//! same documents: a change to the co-editing core that alters what Docs does fails here.
//!
//! `cargo test -p wordcraft-collab --test parity -- --ignored record_parity_log` writes the
//! fixture again (only when a change of behaviour is intended).

mod support;

use serde_json::{Value, json};
use support::{Peer, Relay, doc_with, keep_version};
use wordcraft_collab::hash::doc_hash;
use wordcraft_collab::proto::{HistEntry, Op, Outbound};
use wordcraft_collab::replica::Replica;
use wordcraft_engine::with_clock;

const SEED: u64 = 0x5EED_C011AB;
const PEOPLE: usize = 5;
const EDITS: usize = 2_000;
const BASE: &str = "The quick brown fox jumps over the lazy dog.";
/// A document fingerprint every this many revisions (and at the end).
const EVERY: usize = 250;
const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/parity-2k.json.z");
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

/// One person does one thing, chosen with the weights of `tests/convergence.rs`, plus the paths
/// the replica takes besides plain edits: restoring a version, a refused edit and a resync from
/// another replica's snapshot. Local edits run at the recorder's clock; deliveries replay each
/// edit at its own time.
fn step(rng: &mut Rng, peers: &mut [Peer], relay: &mut Relay, clock: u64) {
    let i = rng.below(PEOPLE);
    let text = TEXTS[rng.below(TEXTS.len())];
    match rng.below(29) {
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
        // Restore the version this person's confirmed document is.
        22 => {
            let v = keep_version(relay, &peers[i]);
            peers[i].replica.restore(v);
        }
        // Everything settles, then this person starts again from the next person's snapshot.
        24 => resync(peers, relay, i, (i + 1) % PEOPLE),
        k => {
            let p = &mut peers[i];
            match k {
                0..=3 => with_clock(clock, || p.type_text(text)),
                4..=5 => {
                    let _ = with_clock(clock, || p.visible.run("text.insert", &json!({ "text": text })));
                }
                6 => with_clock(clock, || p.act("text.newParagraph", json!({}))),
                7 => with_clock(clock, || p.act("text.backspace", json!({}))),
                8 => with_clock(clock, || p.act("text.delete", json!({}))),
                9..=10 => {
                    let id = if rng.below(2) == 0 { "caret.left" } else { "caret.right" };
                    for _ in 0..rng.below(6) + 1 {
                        p.act(id, json!({}));
                    }
                }
                11 => with_clock(clock, || {
                    p.act("select.word", json!({}));
                    p.act("format.bold", json!({}));
                }),
                12 => with_clock(clock, || {
                    p.act("select.word", json!({}));
                    p.type_text(text);
                }),
                13 => with_clock(clock, || {
                    p.act("select.word", json!({}));
                    p.act("review.newComment", json!({"text": "note"}));
                }),
                14 => with_clock(clock, || p.act("insert.table", json!({"rows": 2, "cols": 2}))),
                15..=16 => with_clock(clock, || p.act("edit.undo", json!({}))),
                17 => with_clock(clock, || p.act("edit.redo", json!({}))),
                // The relay refuses this person's next edit.
                23 => {
                    p.replica.drain_local(&mut p.visible);
                    if let Some(op) = p.replica.next_to_send() {
                        p.replica.refused(op.seq, &mut p.visible);
                    }
                }
                _ => p.deliver(rng.below(16) + 1),
            }
        }
    }
    fresh_resyncs(peers, relay);
}

/// Everything settles, then person `i` starts again from person `j`'s snapshot, read back from
/// its JSON as the relay stores it.
fn resync(peers: &mut [Peer], relay: &mut Relay, i: usize, j: usize) {
    let mut refs: Vec<&mut Peer> = peers.iter_mut().collect();
    relay.settle(&mut refs);
    let Ok(Outbound::Snapshot { rev, doc, history }) = peers[j].replica.snapshot() else { panic!("snapshot") };
    let history: Vec<HistEntry> = serde_json::from_str(&serde_json::to_string(&history).unwrap()).unwrap();
    let doc = wordcraft_engine::io::open_bytes("doc.json", doc.as_bytes()).unwrap();
    let client = format!("c{i}");
    let seen = relay.log.iter().filter(|op| op.client == client).map(|op| op.seq).max();
    let p = &mut peers[i];
    p.replica.resume(rev, Some((doc, history)), Vec::new(), seen, &mut p.visible).unwrap();
}

/// As the relay does when a replica asks for a fresh resync (an undo reached past what it holds
/// since it last started from a snapshot): it starts again from someone else's.
fn fresh_resyncs(peers: &mut [Peer], relay: &mut Relay) {
    let stale: Vec<usize> = (0..PEOPLE).filter(|&i| peers[i].replica.take_needs_fresh()).collect();
    if stale.is_empty() {
        return;
    }
    let Some(j) = (0..PEOPLE).find(|k| !stale.contains(k)) else { panic!("everyone needs a fresh snapshot") };
    for i in stale {
        resync(peers, relay, i, j);
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
        step(&mut rng, &mut peers, &mut relay, clock);
    }
    let mut refs: Vec<&mut Peer> = peers.iter_mut().collect();
    relay.settle(&mut refs);
    fresh_resyncs(&mut peers, &mut relay);
    let log = relay.log.clone();
    // Everyone ended with the same document, the one a replay of the log gives.
    let end = hash_after(&log, log.len());
    for (i, p) in peers.iter().enumerate() {
        assert_eq!(doc_hash(p.replica.confirmed()), end, "person {i}'s confirmed document");
        assert_eq!(p.text(), p.confirmed_text(), "person {i} sees what is confirmed");
    }
    let mut marks: Vec<usize> = (EVERY..=log.len()).step_by(EVERY).collect();
    if marks.last() != Some(&log.len()) {
        marks.push(log.len());
    }
    let hashes: Vec<Value> = marks.iter().map(|&n| json!({"rev": n, "hash": hash_after(&log, n)})).collect();
    let wire: Vec<String> = log.iter().map(|op| serde_json::to_string(op).unwrap()).collect();
    json!({"seed": SEED, "people": PEOPLE, "base": BASE, "wire": wire, "hashes": hashes})
}

fn fixture() -> Value {
    let packed = std::fs::read(FIXTURE).expect("tests/fixtures/parity-2k.json.z (record it with --ignored record_parity_log)");
    let bytes = miniz_oxide::inflate::decompress_to_vec_zlib(&packed).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
#[ignore]
fn record_parity_log() {
    let log = record();
    std::fs::create_dir_all(std::path::Path::new(FIXTURE).parent().unwrap()).unwrap();
    let text = serde_json::to_string(&log).unwrap() + "\n";
    std::fs::write(FIXTURE, miniz_oxide::deflate::compress_to_vec_zlib(text.as_bytes(), 9)).unwrap();
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
