//! Co-editing under load (co-editing spec §11): 20 people typing 5 characters a second each in a
//! 188-page document, seen from one of them. Each frame runs what the browser frame runs — the
//! relay's messages through [`CollabLink`], the editor's logic and drawing, the link's tick and
//! the page events — and is timed on the CPU (the GPU's work is not counted).
//!
//! `cargo test -p wordcraft-web --release coediting_load_bench -- --ignored --nocapture`

use std::collections::VecDeque;
use std::time::Instant;

use serde_json::{Value, json};
use wordcraft_collab::proto::{Anchor, Inbound, Op, OpKind};
use wordcraft_doc::offsets::linear_text;
use wordcraft_doc::{Document, StoryRef};
use wordcraft_engine::{RunCtx, Session};
use wordcraft_ui_egui::{Services, WordApp};

use crate::collab_link::CollabLink;

const FPS: f64 = 60.0;
const PEOPLE: usize = 20;
const CHARS_PER_SECOND: f64 = 5.0;
const PAGES: usize = 188;
const SCREEN: egui::Vec2 = egui::vec2(1280.0, 800.0);

/// The built-in sample repeated until it fills `pages` pages.
pub(crate) fn long_document(pages: usize) -> Document {
    let sample = wordcraft_engine::sample::sample_document();
    let mut doc = sample.clone();
    loop {
        let mut s = Session::new(doc.clone());
        if s.layout().pages.len() >= pages {
            return doc;
        }
        doc.body.extend(sample.body.iter().cloned());
    }
}

/// Someone else typing: where their caret is (body offset) and their next edit's number.
struct Typist {
    client: String,
    name: String,
    at: usize,
    seq: u64,
    /// When (seconds) they type next.
    next: f64,
}

/// The relay as this editor sees it: it numbers every edit and sends it back to everyone.
struct Relay {
    rev: u64,
    typists: Vec<Typist>,
    /// Messages for this editor, delivered at the next frame.
    inbox: VecDeque<String>,
    /// Characters everyone typed, for the end check.
    typed: usize,
}

impl Relay {
    /// Number an edit and queue it for this editor; everyone after the place it changed moves.
    fn stamp(&mut self, mut op: Op) {
        self.rev += 1;
        op.rev = Some(self.rev);
        op.user = Some(op.ctx.author.clone());
        let len = op.params.get("text").and_then(Value::as_str).map_or(0, str::len);
        let at = op.sel.anchor.min(op.sel.head);
        for t in &mut self.typists {
            if t.client != op.client && t.at >= at {
                t.at += len;
            }
        }
        self.typed += op.params.get("text").and_then(Value::as_str).map_or(0, |s| s.chars().count());
        self.inbox.push_back(serde_json::to_string(&Inbound::Op(Box::new(op))).unwrap());
    }

    /// Everyone whose turn it is types one character, then says where their caret is.
    fn type_until(&mut self, now: f64) {
        let period = 1.0 / CHARS_PER_SECOND;
        for i in 0..self.typists.len() {
            while self.typists[i].next <= now {
                let t = &mut self.typists[i];
                t.next += period;
                t.seq += 1;
                // Words of five letters and a space.
                let ch = if t.seq.is_multiple_of(6) { " " } else { &"wordc"[(t.seq % 6) as usize - 1..(t.seq % 6) as usize] };
                let ctx = RunCtx { author: t.name.clone(), time: 1_790_000_000, ..Default::default() };
                let op = Op {
                    client: t.client.clone(),
                    seq: t.seq,
                    base: self.rev,
                    kind: OpKind::Cmd,
                    cmd: "text.insert".into(),
                    params: json!({ "text": ch }),
                    sel: Anchor { story: StoryRef::Body, anchor: t.at, head: t.at },
                    ctx,
                    rev: None,
                    user: None,
                };
                let at = t.at;
                self.stamp(op);
                let t = &mut self.typists[i];
                t.at = at + 1;
                let presence = json!({"t": "Presence", "client": t.client, "user": t.name, "color": i % 8, "rev": self.rev,
                    "sel": {"story": "body", "anchor": t.at, "head": t.at}});
                self.inbox.push_back(presence.to_string());
            }
        }
    }
}

/// Places for the others to type, spread evenly through the document, each inside a word.
fn spread(text: &str, n: usize) -> Vec<usize> {
    (1..=n)
        .map(|i| {
            let mut at = text.len() * i / (n + 1);
            while !text.is_char_boundary(at) || !text[at..].starts_with(|c: char| c.is_alphabetic()) {
                at += 1;
            }
            at
        })
        .collect()
}

struct Stats {
    mean: f64,
    median: f64,
    p95: f64,
    max: f64,
}

fn stats(mut v: Vec<f64>) -> Stats {
    v.sort_by(f64::total_cmp);
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    Stats { mean: v.iter().sum::<f64>() / v.len() as f64, median: at(0.5), p95: at(0.95), max: at(1.0) }
}

/// Run `seconds` of co-editing and return each frame's time in ms.
fn run(doc: &Document, seconds: f64) -> (Vec<f64>, Relay, WordApp, CollabLink) {
    let snapshot = String::from_utf8(wordcraft_engine::io::save_bytes("d.json", doc).unwrap()).unwrap();
    let text = linear_text(doc, StoryRef::Body);
    let typists = spread(&text, PEOPLE - 1)
        .into_iter()
        .enumerate()
        .map(|(i, at)| Typist {
            client: format!("c{i}"),
            name: format!("Person {i}"),
            at,
            seq: 0,
            next: i as f64 / (CHARS_PER_SECOND * (PEOPLE - 1) as f64),
        })
        .collect();
    let mut relay = Relay { rev: 0, typists, inbox: VecDeque::new(), typed: 0 };
    let welcome = json!({"t": "Welcome", "rev": 0, "snapshot": snapshot, "ops": [], "you": "me", "role": "edit",
        "peers": relay.typists.iter().enumerate().map(|(i, t)| json!({"client": t.client, "user": t.name, "color": i % 8})).collect::<Vec<_>>()});
    relay.inbox.push_back(welcome.to_string());

    let mut app = WordApp::new(Session::new(Document::new()), Services::default());
    app.session.author = "Me".into();
    let mut link = CollabLink::new(Some("sample.docx".into()));
    let mut watcher = crate::api::Watcher::default();
    let mut batch = crate::remote_batch::RemoteBatch::default();
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN);
    let warmup = 60;
    let frames = warmup + (seconds * FPS) as usize;
    let mut times = Vec::new();
    let mut my_next = 0.0;
    let mut parts = [0.0f64; 9];
    for f in 0..frames {
        let now = f as f64 / FPS;
        let typing = f >= warmup;
        if typing {
            relay.type_until(now - warmup as f64 / FPS);
        }
        let start = Instant::now();
        let mut outbound = Vec::new();
        // Like `WebShell::logic`: the relay's messages (batched), the editor, its own typing, the tick.
        let input = egui::RawInput { screen_rect: Some(screen), time: Some(now), ..Default::default() };
        let out = ctx.run_ui(input, |ui| {
            let mut t = Instant::now();
            let mut lap = |i: usize, t: &mut Instant| {
                if typing {
                    parts[i] += t.elapsed().as_secs_f64() * 1e3;
                }
                *t = Instant::now();
            };
            for msg in relay.inbox.drain(..) {
                batch.push(msg);
            }
            for msg in batch.release(now * 1000.0) {
                outbound.extend(link.on_message(&msg, &mut app.session));
            }
            lap(0, &mut t);
            if link.take_document_ready() {
                app.ui.backstage = false;
            }
            app.logic(ui.ctx());
            lap(1, &mut t);
            if typing && now >= my_next {
                my_next = now + 1.0 / CHARS_PER_SECOND;
                let _ = app.session.run("text.insert", &json!({"text": "m"}));
            }
            lap(2, &mut t);
            outbound.extend(link.tick(&mut app.session, now * 1000.0));
            lap(3, &mut t);
            app.peers = link
                .peers(&app.session)
                .into_iter()
                .map(|p| wordcraft_ui_egui::RemotePeer { typing: p.typing(now * 1000.0), name: p.name, color: p.color, sel: p.sel })
                .collect();
            lap(4, &mut t);
            app.collab.active = link.rev().is_some();
            let people: Vec<(String, u8)> = app.peers.iter().map(|p| (p.name.clone(), p.color)).collect();
            let edited_by = link.take_edited_by();
            let _ = watcher.events(&app, link.rev().unwrap_or(0), &edited_by, &people);
            lap(5, &mut t);
            let _ = app.session.layout();
            lap(8, &mut t);
            app.ui(ui);
            lap(6, &mut t);
        });
        let t = Instant::now();
        let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
        if typing {
            parts[7] += t.elapsed().as_secs_f64() * 1e3;
        }
        let elapsed = start.elapsed().as_secs_f64() * 1e3;
        if typing {
            times.push(elapsed);
        }
        // This editor's own edits: the relay numbers them and sends them back.
        for msg in outbound {
            let v: Value = serde_json::from_str(&msg).unwrap();
            if v["t"] == "Op" {
                relay.stamp(serde_json::from_value(v).unwrap());
            }
        }
    }
    // Everyone stops: what is still on its way arrives.
    for f in frames..frames + 30 {
        let now = f as f64 / FPS;
        let input = egui::RawInput { screen_rect: Some(screen), time: Some(now), ..Default::default() };
        let mut outbound = Vec::new();
        let _ = ctx.run_ui(input, |ui| {
            for msg in relay.inbox.drain(..) {
                batch.push(msg);
            }
            for msg in batch.release(now * 1000.0) {
                outbound.extend(link.on_message(&msg, &mut app.session));
            }
            outbound.extend(link.tick(&mut app.session, now * 1000.0));
            app.ui(ui);
        });
        for msg in outbound {
            let v: Value = serde_json::from_str(&msg).unwrap();
            if v["t"] == "Op" {
                relay.stamp(serde_json::from_value(v).unwrap());
            }
        }
    }
    let n = times.len() as f64;
    let names = ["messages", "logic", "own typing", "tick", "peers", "events", "draw", "tessellate", "layout"];
    eprintln!("per frame: {}", names.iter().zip(parts).map(|(k, v)| format!("{k} {:.2} ms", v / n)).collect::<Vec<_>>().join(", "));
    (times, relay, app, link)
}

/// The spec's check: each editor stays under 16 ms a frame on average.
#[test]
#[ignore]
fn coediting_load_bench() {
    let doc = long_document(PAGES);
    let pages = Session::new(doc.clone()).layout().pages.len();
    let before = linear_text(&doc, StoryRef::Body).chars().count();
    let (times, relay, app, link) = run(&doc, 10.0);
    let s = stats(times.clone());
    eprintln!(
        "{pages} pages, {PEOPLE} people × {CHARS_PER_SECOND} chars/s, {} frames, {} edits: mean {:.2} ms, median {:.2}, p95 {:.2}, max {:.2}",
        times.len(),
        relay.rev,
        s.mean,
        s.median,
        s.p95,
        s.max
    );
    assert!(pages >= PAGES, "{pages} pages");
    assert_eq!(link.pending(), 0, "every edit of this person's was confirmed");
    let after = linear_text(&app.session.doc, StoryRef::Body).chars().count();
    assert_eq!(after, before + relay.typed, "every character typed is in the document");
    assert!(s.mean < 16.0, "average frame {:.2} ms", s.mean);
}

#[test]
#[ignore]
fn relayout_probe() {
    let doc = long_document(PAGES);
    let mut s = Session::new(doc);
    let t = Instant::now();
    let n = s.layout().pages.len();
    eprintln!("cold {n} pages: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    for (name, cmd) in [("start", "caret.docStart"), ("end", "caret.docEnd")] {
        s.run(cmd, &json!({})).unwrap();
        for _ in 0..3 {
            s.run("text.insert", &json!({"text": "x"})).unwrap();
            let t = Instant::now();
            let _ = s.layout();
            eprintln!("type at {name}: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
        }
    }
    // A remote edit: the document replaced by an edited copy.
    let mut other = Session::new(s.doc.clone());
    other.run("caret.docEnd", &json!({})).unwrap();
    other.run("text.insert", &json!({"text": "y"})).unwrap();
    s.doc = other.doc.clone();
    s.touch();
    let t = Instant::now();
    let _ = s.layout();
    eprintln!("remote at end: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
}
