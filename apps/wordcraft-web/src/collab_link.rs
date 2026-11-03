//! Co-editing glue between the page and the replica (`?collab=1`): the suite's link layer
//! (`craft_collab_link`) on a WordCraft document (`wordcraft_collab::link`). The page relays
//! messages between this frame and the relay as JSON strings; this turns them into replica calls
//! on the visible session and returns what to send back. No browser APIs here, so it is tested
//! natively.

pub use wordcraft_collab::link::{CollabLink, HostRequest};
#[cfg(test)]
use wordcraft_collab::link::{OFFLINE_LIMIT_MS, PENDING_LIMIT};

/// A kept version's document for its preview (native JSON in; `None` when it can't be read).
pub fn version_doc(json: &str) -> Option<wordcraft_doc::Document> {
    wordcraft_engine::io::open_bytes("version.json", json.as_bytes()).ok()
}

/// Test helper: the messages from a tick other than presence.
#[cfg(test)]
fn edits_only(out: Vec<String>) -> Vec<String> {
    out.into_iter().filter(|m| !m.contains(r#""t":"Presence""#)).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use wordcraft_doc::{Document, StoryRef};
    use wordcraft_engine::Session;

    use super::*;

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    fn snapshot_of(text: &str) -> String {
        let mut s = Session::new(Document::new());
        s.run("text.insert", &json!({"text": text})).unwrap();
        String::from_utf8(wordcraft_engine::io::save_bytes("d.json", &s.doc).unwrap()).unwrap()
    }

    fn welcome(rev: u64, snapshot: Option<String>, you: &str) -> String {
        json!({"t": "Welcome", "rev": rev, "snapshot": snapshot, "ops": [], "you": you}).to_string()
    }

    #[test]
    fn first_person_uploads_the_opened_document_as_revision_0() {
        let mut link = CollabLink::new(Some("report.docx".into()));
        let mut s = Session::new(Document::new());
        assert!(link.on_message(&welcome(0, None, "c1"), &mut s).is_empty());
        assert!(super::edits_only(link.tick(&mut s, 0.0)).is_empty(), "waits for the document");
        // The host opens the file.
        let mut opened = Session::new(Document::new());
        opened.run("text.insert", &json!({"text": "From file"})).unwrap();
        s.set_document(opened.doc.clone());
        let out = super::edits_only(link.tick(&mut s, 0.0));
        assert_eq!(out.len(), 1);
        let v = parse(&out[0]);
        assert_eq!((v["t"].as_str(), v["rev"].as_u64()), (Some("Snapshot"), Some(0)));
        assert!(s.collab());
        assert!(link.take_document_ready());
    }

    #[test]
    fn joiner_shows_the_snapshot_named_as_the_document() {
        let mut link = CollabLink::new(Some("report.docx".into()));
        let mut s = Session::new(Document::new());
        let id = s.document_id();
        link.on_message(&welcome(4, Some(snapshot_of("Shared")), "c2"), &mut s);
        assert_eq!(s.doc.plain_text(StoryRef::Body), "Shared");
        assert_ne!(s.document_id(), id, "a real document, not the start screen's blank");
        assert_eq!(s.path.as_deref(), Some(std::path::Path::new("report.docx")));
        assert!(s.collab());
        assert!(link.take_document_ready());
        assert_eq!(link.rev(), Some(4));
    }

    #[test]
    fn local_edits_go_out_one_at_a_time_and_echoes_release_the_next() {
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        link.on_message(&welcome(0, Some(snapshot_of("x")), "c1"), &mut s);
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": " "})).unwrap();
        s.run("text.insert", &json!({"text": "."})).unwrap();
        let out = super::edits_only(link.tick(&mut s, 0.0));
        assert_eq!(out.len(), 1);
        assert_eq!(link.pending(), 2);
        let mut op = parse(&out[0]);
        assert_eq!(op["t"], "Op");
        assert!(super::edits_only(link.tick(&mut s, 0.0)).is_empty(), "one in flight");
        op["rev"] = json!(1);
        link.on_message(&op.to_string(), &mut s);
        let next = super::edits_only(link.tick(&mut s, 0.0));
        assert_eq!(parse(&next[0])["params"]["text"], ".");
    }

    #[test]
    fn a_gap_asks_for_a_resync() {
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        link.on_message(&welcome(0, Some(snapshot_of("x")), "c1"), &mut s);
        let op = json!({"t": "Op", "client": "c9", "seq": 1, "base": 0, "kind": "cmd", "cmd": "text.insert", "params": {"text": "z"},
            "sel": {"story": "body", "anchor": 0, "head": 0}, "ctx": {}, "rev": 3});
        let out = link.on_message(&op.to_string(), &mut s);
        assert_eq!(parse(&out[0]), json!({"t": "ResyncRequest", "have": 0}));
    }

    #[test]
    fn snapshot_request_and_save_request() {
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        link.on_message(&welcome(0, Some(snapshot_of("x")), "c1"), &mut s);
        let out = link.on_message(r#"{"t":"SnapshotRequest"}"#, &mut s);
        assert_eq!(parse(&out[0])["t"], "Snapshot");
        assert!(!link.take_save_request());
        assert!(link.on_message(r#"{"t":"Save"}"#, &mut s).is_empty());
        assert!(link.take_save_request());
        assert!(!link.take_save_request());
        assert_eq!(parse(&link.saved()), json!({"t": "Saved", "rev": 0}));
    }

    #[test]
    fn bad_and_oversized_messages_are_ignored() {
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        assert!(link.on_message("nonsense", &mut s).is_empty());
        let big = json!({"t": "Op", "params": "x".repeat(300 * 1024)}).to_string();
        assert!(link.on_message(&big, &mut s).is_empty());
        assert_eq!(link.rev(), None);
    }

    #[test]
    fn reconnect_welcome_resumes_and_resends_pending() {
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        link.on_message(&welcome(0, Some(snapshot_of("x")), "c1"), &mut s);
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "!"})).unwrap();
        let typed = s.doc.plain_text(StoryRef::Body);
        assert_eq!(super::edits_only(link.tick(&mut s, 0.0)).len(), 1);
        // The connection drops before the echo; the relay never got it.
        let out = link.on_message(&welcome(0, None, "c1"), &mut s);
        assert_eq!(out.len(), 1);
        assert_eq!(parse(&out[0])["params"]["text"], "!");
        assert_eq!(s.doc.plain_text(StoryRef::Body), typed);
    }
}

#[cfg(test)]
mod seen_tests {
    use serde_json::json;
    use wordcraft_doc::{Document, StoryRef};
    use wordcraft_engine::Session;

    use super::CollabLink;

    #[test]
    fn a_resync_snapshot_holding_an_edit_does_not_resend_it() {
        let mut s0 = Session::new(Document::new());
        s0.run("text.insert", &json!({"text": "ab"})).unwrap();
        let snap = |s: &Session| String::from_utf8(wordcraft_engine::io::save_bytes("d.json", &s.doc).unwrap()).unwrap();
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        link.on_message(&json!({"t": "Welcome", "rev": 0, "snapshot": snap(&s0), "ops": [], "you": "c1"}).to_string(), &mut s);
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "X"})).unwrap();
        assert_eq!(super::edits_only(link.tick(&mut s, 0.0)).len(), 1);
        // The relay took it into a snapshot (seen = 1) before this replica heard back.
        let mut s1 = Session::new(Document::new());
        s1.run("text.insert", &json!({"text": "abX"})).unwrap();
        let out = link.on_message(&json!({"t": "Resync", "rev": 1, "snapshot": snap(&s1), "ops": [], "seen": 1}).to_string(), &mut s);
        assert!(out.is_empty(), "nothing to resend: {out:?}");
        assert_eq!(s.doc.plain_text(StoryRef::Body), "abX");
    }
}

#[cfg(test)]
mod recovery_tests {
    use serde_json::json;
    use wordcraft_doc::{Document, StoryRef};
    use wordcraft_engine::Session;

    use super::CollabLink;

    fn snap(text: &str) -> String {
        let mut s = Session::new(Document::new());
        s.run("text.insert", &json!({"text": text})).unwrap();
        String::from_utf8(wordcraft_engine::io::save_bytes("d.json", &s.doc).unwrap()).unwrap()
    }

    /// A joiner whose Welcome couldn't be used asks for a Resync, and the Resync then
    /// starts it (there's no replica yet).
    #[test]
    fn a_resync_starts_a_replica_that_could_not_join() {
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        link.on_message(&json!({"t": "Welcome", "rev": 0, "snapshot": "not a document", "ops": [], "you": "c1"}).to_string(), &mut s);
        assert_eq!(link.rev(), None);
        link.on_message(&json!({"t": "Resync", "rev": 2, "snapshot": snap("Shared"), "ops": []}).to_string(), &mut s);
        assert_eq!(link.rev(), Some(2));
        assert_eq!(s.doc.plain_text(StoryRef::Body), "Shared");
        assert!(link.take_document_ready());
    }

    /// An op no replica can decode, at the next revision, is skipped.
    #[test]
    fn an_unreadable_op_at_the_next_revision_is_skipped() {
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        link.on_message(&json!({"t": "Welcome", "rev": 0, "snapshot": snap("x"), "ops": [], "you": "c1"}).to_string(), &mut s);
        let out = link.on_message(&json!({"t": "Op", "rev": 1, "sel": "garbage"}).to_string(), &mut s);
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(link.rev(), Some(1));
    }

    /// An edit dropped after someone else's change is reported.
    #[test]
    fn a_dropped_edit_leaves_a_note() {
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        link.on_message(&json!({"t": "Welcome", "rev": 0, "snapshot": snap("Intro"), "ops": [], "you": "c1"}).to_string(), &mut s);
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("text.newParagraph", &json!({})).unwrap();
        s.run("insert.table", &json!({"rows": 2, "cols": 2})).unwrap();
        // Simulate: this replica's two edits are confirmed, then its row edit waits while
        // someone else's delete of everything arrives.
        let mut rev = 0;
        for _ in 0..2 {
            let out = super::edits_only(link.tick(&mut s, 0.0));
            let mut op: serde_json::Value = serde_json::from_str(&out[0]).unwrap();
            rev += 1;
            op["rev"] = json!(rev);
            link.on_message(&op.to_string(), &mut s);
        }
        s.run("table.insertRowBelow", &json!({})).unwrap();
        // Someone else deletes everything (the row edit hasn't been sent yet).
        let other = json!({"t": "Op", "client": "c9", "seq": 1, "base": rev, "kind": "cmd", "cmd": "text.delete", "params": {},
            "sel": {"story": "body", "anchor": 0, "head": 100000}, "ctx": {}, "rev": rev + 1});
        link.on_message(&other.to_string(), &mut s);
        let note = link.take_note().expect("a note");
        assert!(note.contains("couldn't be applied"), "{note}");
    }
}

#[cfg(test)]
mod presence_tests {
    use serde_json::{Value, json};
    use wordcraft_doc::{Document, StoryRef};
    use wordcraft_engine::Session;

    use super::CollabLink;

    fn snap(text: &str) -> String {
        let mut s = Session::new(Document::new());
        s.run("text.insert", &json!({"text": text})).unwrap();
        String::from_utf8(wordcraft_engine::io::save_bytes("d.json", &s.doc).unwrap()).unwrap()
    }

    fn joined(text: &str, peers: Value) -> (CollabLink, Session) {
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        link.on_message(&json!({"t": "Welcome", "rev": 0, "snapshot": snap(text), "ops": [], "you": "me", "peers": peers}).to_string(), &mut s);
        link.tick(&mut s, 0.0);
        (link, s)
    }

    fn presence(client: &str, user: &str, anchor: usize) -> String {
        json!({"t": "Presence", "client": client, "user": user, "color": 3, "rev": 0, "sel": {"story": "body", "anchor": anchor, "head": anchor}})
            .to_string()
    }

    fn caret_off(link: &CollabLink, s: &Session, client: &str) -> usize {
        link.peers(s).into_iter().find(|p| p.client == client).and_then(|p| p.sel).map(|sel| sel.focus.off).unwrap()
    }

    #[test]
    fn join_presence_leave_update_the_book() {
        let (mut link, mut s) = joined("Hello world", json!([{"client": "a", "user": "Ann", "color": 0}]));
        assert_eq!(link.peers(&s).len(), 1);
        assert_eq!(link.peers(&s)[0].name, "Ann");
        link.on_message(&json!({"t": "Join", "client": "b", "user": "Bob", "color": 1}).to_string(), &mut s);
        link.on_message(&presence("b", "Bob", 6), &mut s);
        assert_eq!(caret_off(&link, &s, "b"), 6);
        link.on_message(&json!({"t": "Leave", "client": "a"}).to_string(), &mut s);
        let names: Vec<String> = link.peers(&s).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Bob"]);
    }

    #[test]
    fn presence_sent_only_when_selection_changes_and_at_most_ten_per_second() {
        let (mut link, mut s) = joined("Hello world", json!([]));
        let sent = |out: &[String]| out.iter().filter(|m| m.contains(r#""t":"Presence""#)).count();
        s.run("caret.docEnd", &json!({})).unwrap();
        assert_eq!(sent(&link.tick(&mut s, 1000.0)), 1);
        assert_eq!(sent(&link.tick(&mut s, 1500.0)), 0, "unchanged: nothing sent");
        s.run("caret.left", &json!({})).unwrap();
        assert_eq!(sent(&link.tick(&mut s, 1550.0)), 1, "moved, and the last one was 550 ms ago");
        s.run("caret.left", &json!({})).unwrap();
        assert_eq!(sent(&link.tick(&mut s, 1600.0)), 0, "within 100 ms of the last one");
        assert!(link.presence_pending());
        assert_eq!(sent(&link.tick(&mut s, 1651.0)), 1);
        assert!(!link.presence_pending());
    }

    #[test]
    fn peer_caret_moves_with_text_typed_before_it() {
        let (mut link, mut s) =
            joined("Hello world", json!([{"client": "a", "user": "Ann", "color": 0, "rev": 0, "sel": {"story": "body", "anchor": 6, "head": 6}}]));
        s.run("caret.docStart", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "Hi "})).unwrap();
        link.tick(&mut s, 10.0);
        assert_eq!(caret_off(&link, &s, "a"), 9);
    }

    #[test]
    fn peer_caret_survives_deletion_around_it() {
        let (mut link, mut s) = joined(
            "Hello big world",
            json!([{"client": "a", "user": "Ann", "color": 0, "rev": 0, "sel": {"story": "body", "anchor": 8, "head": 8}}]),
        );
        s.run("select.text", &json!({"text": "big "})).unwrap();
        s.run("text.delete", &json!({})).unwrap();
        link.tick(&mut s, 10.0);
        assert_eq!(caret_off(&link, &s, "a"), 6);
        assert_eq!(s.doc.plain_text(StoryRef::Body), "Hello world");
    }

    #[test]
    fn a_peer_typing_is_marked_for_three_seconds() {
        let (mut link, mut s) = joined("x", json!([{"client": "a", "user": "Ann", "color": 0}]));
        let op = json!({"t": "Op", "client": "a", "seq": 1, "base": 0, "kind": "cmd", "cmd": "text.insert", "params": {"text": "y"},
            "sel": {"story": "body", "anchor": 1, "head": 1}, "ctx": {}, "rev": 1});
        link.tick(&mut s, 5000.0);
        link.on_message(&op.to_string(), &mut s);
        link.tick(&mut s, 5001.0);
        assert!(link.peers(&s)[0].typing(6000.0));
        assert!(!link.peers(&s)[0].typing(8100.0));
    }

    #[test]
    fn peers_survive_a_reconnect_welcome() {
        let (mut link, mut s) = joined("x", json!([{"client": "a", "user": "Ann", "color": 0}]));
        link.on_message(
            &json!({"t": "Welcome", "rev": 0, "snapshot": null, "ops": [], "you": "me", "peers": [{"client": "b", "user": "Bob", "color": 1}]})
                .to_string(),
            &mut s,
        );
        let names: Vec<String> = link.peers(&s).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Bob"], "the reconnect's list replaces the old one");
    }
}

#[cfg(test)]
mod presence_order_tests {
    use serde_json::json;
    use wordcraft_doc::Document;
    use wordcraft_engine::Session;

    use super::CollabLink;

    fn snap(text: &str) -> String {
        let mut s = Session::new(Document::new());
        s.run("text.insert", &json!({"text": text})).unwrap();
        String::from_utf8(wordcraft_engine::io::save_bytes("d.json", &s.doc).unwrap()).unwrap()
    }

    fn joined(text: &str) -> (CollabLink, Session) {
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        let peers = json!([{"client": "a", "user": "Ann", "color": 0}]);
        link.on_message(&json!({"t": "Welcome", "rev": 0, "snapshot": snap(text), "ops": [], "you": "me", "peers": peers}).to_string(), &mut s);
        link.tick(&mut s, 0.0);
        (link, s)
    }

    fn insert_op(rev: u64, at: usize, text: &str) -> String {
        json!({"t": "Op", "client": "a", "seq": rev, "base": rev - 1, "kind": "cmd", "cmd": "text.insert", "params": {"text": text},
            "sel": {"story": "body", "anchor": at, "head": at}, "ctx": {}, "rev": rev})
        .to_string()
    }

    fn ann(link: &CollabLink, s: &Session) -> usize {
        link.peers(s)[0].sel.as_ref().map(|sel| sel.focus.off).unwrap()
    }

    #[test]
    fn a_typing_peer_caret_follows_their_typing() {
        let (mut link, mut s) = joined("Hello");
        link.on_message(&insert_op(1, 5, " there"), &mut s);
        link.tick(&mut s, 10.0);
        assert_eq!(ann(&link, &s), 11, "after what Ann typed");
    }

    #[test]
    fn presence_waits_for_the_edits_it_follows() {
        let (mut link, mut s) = joined("Hello");
        // Ann typed " there" (revision 1) and says she's after it, but her edit hasn't arrived yet.
        link.on_message(
            &json!({"t": "Presence", "client": "a", "user": "Ann", "color": 0, "rev": 1, "sel": {"story": "body", "anchor": 11, "head": 11}})
                .to_string(),
            &mut s,
        );
        link.tick(&mut s, 10.0);
        link.on_message(&insert_op(1, 5, " there"), &mut s);
        link.tick(&mut s, 20.0);
        assert_eq!(ann(&link, &s), 11, "placed once, not shifted again by her own edit");
    }

    #[test]
    fn presence_from_an_older_revision_moves_through_later_edits() {
        let (mut link, mut s) = joined("Hello world");
        // Someone else's edit (revision 1) puts "Big " at the start; Ann said where she was before it.
        let other = json!({"t": "Op", "client": "b", "seq": 1, "base": 0, "kind": "cmd", "cmd": "text.insert", "params": {"text": "Big "},
            "sel": {"story": "body", "anchor": 0, "head": 0}, "ctx": {}, "rev": 1});
        link.on_message(&other.to_string(), &mut s);
        link.tick(&mut s, 10.0);
        link.on_message(
            &json!({"t": "Presence", "client": "a", "user": "Ann", "color": 0, "rev": 0, "sel": {"story": "body", "anchor": 6, "head": 6}})
                .to_string(),
            &mut s,
        );
        link.tick(&mut s, 20.0);
        assert_eq!(ann(&link, &s), 10);
    }

    #[test]
    fn presence_is_not_sent_while_edits_are_unconfirmed() {
        let (mut link, mut s) = joined("Hello");
        let presence = |out: &[String]| out.iter().find(|m| m.contains(r#""t":"Presence""#)).cloned();
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "!"})).unwrap();
        let out = link.tick(&mut s, 1000.0);
        assert!(presence(&out).is_none(), "an edit is in flight");
        let mut op: serde_json::Value = serde_json::from_str(out.iter().find(|m| m.contains(r#""t":"Op""#)).unwrap()).unwrap();
        op["rev"] = json!(1);
        link.on_message(&op.to_string(), &mut s);
        let p = presence(&link.tick(&mut s, 1200.0)).expect("sent once confirmed");
        let v: serde_json::Value = serde_json::from_str(&p).unwrap();
        assert_eq!(v["rev"], 1);
    }
}

#[cfg(test)]
mod presence_review_tests {
    use serde_json::{Value, json};
    use wordcraft_doc::{Document, StoryRef};
    use wordcraft_engine::Session;

    use super::CollabLink;

    fn snap(text: &str) -> String {
        let mut s = Session::new(Document::new());
        s.run("text.insert", &json!({"text": text})).unwrap();
        String::from_utf8(wordcraft_engine::io::save_bytes("d.json", &s.doc).unwrap()).unwrap()
    }

    fn joined(text: &str, peers: Value) -> (CollabLink, Session) {
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        link.on_message(&json!({"t": "Welcome", "rev": 0, "snapshot": snap(text), "ops": [], "you": "me", "peers": peers}).to_string(), &mut s);
        link.tick(&mut s, 0.0);
        (link, s)
    }

    fn at(client: &str, user: &str, off: usize) -> Value {
        json!({"client": client, "user": user, "color": 0, "rev": 0, "sel": {"story": "body", "anchor": off, "head": off}})
    }

    fn op(client: &str, seq: u64, rev: u64, at: usize, text: &str) -> String {
        json!({"t": "Op", "client": client, "seq": seq, "base": rev - 1, "kind": "cmd", "cmd": "text.insert", "params": {"text": text},
            "sel": {"story": "body", "anchor": at, "head": at}, "ctx": {}, "rev": rev})
        .to_string()
    }

    fn off(link: &CollabLink, s: &Session, client: &str) -> Option<usize> {
        link.peers(s).into_iter().find(|p| p.client == client).and_then(|p| p.sel).map(|sel| sel.focus.off)
    }

    /// C1: a position placed by Presence still moves with the next edit, after idle ticks.
    #[test]
    fn a_placed_peer_moves_with_my_next_edit_after_idle_ticks() {
        let (mut link, mut s) = joined("Hello world", json!([]));
        link.on_message(
            &json!({"t": "Presence", "client": "a", "user": "Ann", "color": 0, "rev": 0, "sel": {"story": "body", "anchor": 6, "head": 6}})
                .to_string(),
            &mut s,
        );
        for t in 1..5 {
            link.tick(&mut s, t as f64 * 10.0);
        }
        s.run("caret.docStart", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "Hi "})).unwrap();
        link.tick(&mut s, 100.0);
        assert_eq!(off(&link, &s, "a"), Some(9));
    }

    /// C2: my typing and someone's edit in the same frame move an idle peer correctly.
    #[test]
    fn an_idle_peer_moves_correctly_when_two_edits_land_in_one_frame() {
        let (mut link, mut s) = joined("Hello world", json!([at("b", "Bob", 6), {"client": "a", "user": "Ann", "color": 1}]));
        // My keystroke (start of the text) and Ann's edit (end) arrive before the next tick.
        s.run("caret.docStart", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "Hi "})).unwrap();
        link.on_message(&op("a", 1, 1, 11, "!!!"), &mut s);
        link.tick(&mut s, 10.0);
        assert_eq!(off(&link, &s, "b"), Some(9), "Bob stays on 'w', shifted only by my 'Hi '");
    }

    /// C2: two remote edits in one frame: the first author is moved by the second edit.
    #[test]
    fn a_typing_peer_is_moved_by_a_later_edit_in_the_same_frame() {
        let (mut link, mut s) =
            joined("Hello world", json!([{"client": "a", "user": "Ann", "color": 0}, {"client": "b", "user": "Bob", "color": 1}]));
        link.on_message(&op("a", 1, 1, 11, "!"), &mut s); // Ann ends at 12
        link.on_message(&op("b", 1, 2, 0, "Big "), &mut s); // Bob inserts 4 before her
        link.tick(&mut s, 10.0);
        assert_eq!(off(&link, &s, "a"), Some(16));
        assert_eq!(off(&link, &s, "b"), Some(4));
    }

    /// C3: one malformed person in a Welcome doesn't stop the newcomer from joining.
    #[test]
    fn a_malformed_peer_in_welcome_is_skipped() {
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        let peers = json!([{"client": "x", "user": "X", "color": 300}, {"client": "y", "user": "Y", "color": 1, "sel": {"story": {"part": 1, "junk": "z"}, "anchor": 0, "head": 0}}, {"client": "a", "user": "Ann", "color": 2}]);
        link.on_message(&json!({"t": "Welcome", "rev": 0, "snapshot": snap("Hi"), "ops": [], "you": "me", "peers": peers}).to_string(), &mut s);
        assert_eq!(link.rev(), Some(0), "joined");
        let names: Vec<String> = link.peers(&s).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Ann"]);
    }

    /// I1: a resync's replayed edits don't leave an author position to be applied on my next echo.
    #[test]
    fn my_echo_never_moves_someone_else() {
        let (mut link, mut s) = joined("Hello world", json!([at("a", "Ann", 3)]));
        // A resync whose tail has Ann's edit (her caret placed by it), then my own edit and echo.
        let tail: Value = serde_json::from_str(&op("a", 1, 1, 11, "!")).unwrap();
        link.on_message(&json!({"t": "Resync", "rev": 0, "snapshot": snap("Hello world"), "ops": [tail]}).to_string(), &mut s);
        link.tick(&mut s, 10.0);
        let before = off(&link, &s, "a");
        s.run("caret.docStart", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "x"})).unwrap();
        let out = link.tick(&mut s, 20.0);
        let mut mine: Value = serde_json::from_str(out.iter().find(|m| m.contains(r#""t":"Op""#)).unwrap()).unwrap();
        mine["rev"] = json!(2);
        link.on_message(&mine.to_string(), &mut s);
        link.tick(&mut s, 30.0);
        assert_eq!(off(&link, &s, "a"), before.map(|o| o + 1), "shifted by my 'x', nothing else");
    }

    /// I2: after my edit is confirmed, my position is sent again even if it equals the last one sent.
    #[test]
    fn my_position_is_resent_after_my_edit_is_confirmed() {
        let (mut link, mut s) = joined("Hello", json!([]));
        let presence = |out: &[String]| out.iter().any(|m| m.contains(r#""t":"Presence""#));
        s.run("caret.docEnd", &json!({})).unwrap();
        assert!(presence(&link.tick(&mut s, 1000.0)));
        s.run("text.insert", &json!({"text": "!"})).unwrap();
        s.run("caret.left", &json!({})).unwrap();
        let out = link.tick(&mut s, 1200.0);
        let mut mine: Value = serde_json::from_str(out.iter().find(|m| m.contains(r#""t":"Op""#)).unwrap()).unwrap();
        mine["rev"] = json!(1);
        link.on_message(&mine.to_string(), &mut s);
        assert!(presence(&link.tick(&mut s, 1400.0)), "others saw me move with my edit: say where I really am");
    }

    /// I4: unconfirmed edits don't keep asking for a tick every 100 ms.
    #[test]
    fn unconfirmed_edits_do_not_keep_presence_pending() {
        let (mut link, mut s) = joined("Hello", json!([]));
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "!"})).unwrap();
        link.tick(&mut s, 1000.0);
        assert!(!link.presence_pending());
    }

    /// I6: someone in a story that doesn't exist has no caret (not one at the top of the body).
    #[test]
    fn a_peer_in_a_missing_story_has_no_position() {
        let (link, s) =
            joined("Hello", json!([{"client": "a", "user": "Ann", "color": 0, "rev": 0, "sel": {"story": {"part": 77}, "anchor": 1, "head": 1}}]));
        assert_eq!(link.peers(&s)[0].sel, None);
        let _ = StoryRef::Body;
    }

    /// I7: a resync keeps people where they were instead of piling them at the end.
    #[test]
    fn a_resync_keeps_people_in_place() {
        let (mut link, mut s) = joined("Hello world", json!([at("a", "Ann", 6)]));
        link.on_message(&json!({"t": "Resync", "rev": 0, "snapshot": snap("Hello world"), "ops": []}).to_string(), &mut s);
        link.tick(&mut s, 10.0);
        assert_eq!(off(&link, &s, "a"), Some(6));
    }
}

#[cfg(test)]
mod api_tests {
    use serde_json::json;
    use wordcraft_doc::{Document, StoryRef};
    use wordcraft_engine::Session;

    use super::CollabLink;

    fn joined() -> (CollabLink, Session) {
        let mut base = Session::new(Document::new());
        base.run("text.insert", &json!({"text": "Hello"})).unwrap();
        let snap = String::from_utf8(wordcraft_engine::io::save_bytes("d.json", &base.doc).unwrap()).unwrap();
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        let peers = json!([{"client": "a", "user": "Ann", "color": 2}]);
        link.on_message(&json!({"t": "Welcome", "rev": 0, "snapshot": snap, "ops": [], "you": "me", "peers": peers}).to_string(), &mut s);
        link.tick(&mut s, 0.0);
        (link, s)
    }

    fn ann_types(rev: u64, at: usize, text: &str) -> String {
        json!({"t": "Op", "client": "a", "seq": rev, "base": rev - 1, "kind": "cmd", "cmd": "text.insert", "params": {"text": text},
            "sel": {"story": "body", "anchor": at, "head": at}, "ctx": {}, "rev": rev})
        .to_string()
    }

    /// Read only (Viewing) refuses this person's edits, not other people's.
    #[test]
    fn viewing_still_receives_remote_edits() {
        let (mut link, mut s) = joined();
        s.run("view.viewing", &json!({"value": true})).unwrap();
        link.on_message(&ann_types(1, 5, "!"), &mut s);
        link.tick(&mut s, 10.0);
        assert_eq!(s.doc.plain_text(StoryRef::Body), "Hello!");
        assert!(s.run("text.insert", &json!({"text": "x"})).is_err());
    }

    #[test]
    fn take_edited_by_names_remote_authors() {
        let (mut link, mut s) = joined();
        assert!(link.take_edited_by().is_empty());
        link.on_message(&ann_types(1, 5, "!"), &mut s);
        assert_eq!(link.take_edited_by(), ["Ann"]);
        assert!(link.take_edited_by().is_empty(), "taken once");
    }
}

#[cfg(test)]
mod viewing_replay_tests {
    use serde_json::json;
    use wordcraft_doc::{Document, StoryRef};
    use wordcraft_engine::Session;

    use super::CollabLink;

    /// Edits made before Viewing was turned on, still waiting for the relay, survive another
    /// person's edit arriving: they are replayed, not refused as new edits.
    #[test]
    fn viewing_keeps_this_persons_unconfirmed_edits() {
        let mut base = Session::new(Document::new());
        base.run("text.insert", &json!({"text": "Hello"})).unwrap();
        let snap = String::from_utf8(wordcraft_engine::io::save_bytes("d.json", &base.doc).unwrap()).unwrap();
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        link.on_message(
            &json!({"t": "Welcome", "rev": 0, "snapshot": snap, "ops": [], "you": "me", "peers": [{"client": "a", "user": "Ann", "color": 1}]})
                .to_string(),
            &mut s,
        );
        link.tick(&mut s, 0.0);
        s.run("caret.docEnd", &json!({})).unwrap();
        for t in [" a", " b", " c"] {
            s.run("text.insert", &json!({"text": t})).unwrap();
            link.tick(&mut s, 1.0);
        }
        s.run("view.viewing", &json!({"value": true})).unwrap();
        // Ann types at the start before the relay confirms anything of ours.
        let ann = json!({"t": "Op", "client": "a", "seq": 1, "base": 0, "kind": "cmd", "cmd": "text.insert", "params": {"text": ">"},
            "sel": {"story": "body", "anchor": 0, "head": 0}, "ctx": {}, "rev": 1});
        link.on_message(&ann.to_string(), &mut s);
        link.tick(&mut s, 2.0);
        assert_eq!(s.doc.plain_text(StoryRef::Body), ">Hello a b c");
        assert!(s.viewing, "still viewing");
        assert!(link.take_note().is_none(), "nothing reported as dropped");
    }
}

#[cfg(test)]
mod protocol_v2_tests {
    use serde_json::{Value, json};
    use wordcraft_doc::{Document, StoryRef};
    use wordcraft_engine::{CollabRole, Session};

    use super::*;

    fn snap(text: &str) -> String {
        let mut s = Session::new(Document::new());
        s.run("text.insert", &json!({"text": text})).unwrap();
        String::from_utf8(wordcraft_engine::io::save_bytes("d.json", &s.doc).unwrap()).unwrap()
    }

    fn joined(text: &str, role: Option<&str>) -> (CollabLink, Session) {
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        let w = json!({"t": "Welcome", "rev": 3, "snapshot": snap(text), "ops": [], "you": "me", "role": role});
        link.on_message(&w.to_string(), &mut s);
        (link, s)
    }

    fn parse(m: &str) -> Value {
        serde_json::from_str(m).unwrap()
    }

    #[test]
    fn the_welcomes_role_limits_the_person() {
        let (_, mut s) = joined("Hi", Some("view"));
        assert_eq!(s.collab_role(), CollabRole::View);
        assert!(s.run("text.insert", &json!({"text": "x"})).is_err());
        let (_, mut s) = joined("Hi", Some("comment"));
        assert_eq!(s.collab_role(), CollabRole::Comment);
        assert!(s.run("text.insert", &json!({"text": "x"})).is_err());
        let (_, s) = joined("Hi", None);
        assert_eq!(s.collab_role(), CollabRole::Edit, "an older relay sends no role");
    }

    #[test]
    fn a_refused_edit_leaves_with_its_reason() {
        let (mut link, mut s) = joined("Hi", None);
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "!"})).unwrap();
        let out = super::edits_only(link.tick(&mut s, 0.0));
        let seq = parse(&out[0])["seq"].as_u64().unwrap();
        link.on_message(&json!({"t": "Refused", "seq": seq, "reason": "You can only comment on this document"}).to_string(), &mut s);
        assert_eq!(s.doc.plain_text(StoryRef::Body), "Hi");
        assert_eq!(link.take_note().as_deref(), Some("You can only comment on this document"));
        assert_eq!(link.pending(), 0);
    }

    /// An export too big for one relay message is refused, not sent (the relay would drop the
    /// connection over it).
    #[test]
    fn a_too_large_export_is_refused() {
        let v = parse(&CollabLink::exported("e1".into(), Ok(vec![0; 38 * 1024 * 1024])));
        assert_eq!((v["ok"].as_bool(), v["data"].is_null()), (Some(false), true));
        assert!(v["error"].as_str().unwrap().contains("too large"));
        let v = parse(&CollabLink::exported("e2".into(), Ok(vec![1, 2, 3])));
        assert_eq!((v["ok"].as_bool(), v["data"].as_str()), (Some(true), Some("AQID")));
    }

    /// A version is the document as this replica has it now, at or after the revision asked
    /// for (others may have edited since the save), never one it hasn't reached.
    #[test]
    fn a_version_is_given_at_or_after_its_revision() {
        let (mut link, mut s) = joined("Kept", None);
        assert!(link.on_message(r#"{"t":"VersionRequest","rev":4}"#, &mut s).is_empty(), "not reached yet");
        for asked in [2, 3] {
            let out = link.on_message(&json!({"t": "VersionRequest", "rev": asked}).to_string(), &mut s);
            let v = parse(&out[0]);
            assert_eq!((v["t"].as_str(), v["rev"].as_u64()), (Some("Version"), Some(3)), "asked {asked}");
            assert!(v["doc"].as_str().unwrap().contains("Kept"));
        }
    }

    #[test]
    fn versions_list_preview_and_restore() {
        let (mut link, mut s) = joined("Now", None);
        assert_eq!(parse(&link.list_versions()), json!({"t": "Versions"}));
        assert_eq!(parse(&link.get_version(2)), json!({"t": "GetVersion", "rev": 2}));
        link.on_message(r#"{"t":"VersionList","versions":[{"rev":2,"at":"2026-10-29T10:00:00Z","users":["Ann"]}]}"#, &mut s);
        assert_eq!(link.take_versions().unwrap()[0].users, ["Ann"]);
        link.on_message(&json!({"t": "VersionDoc", "rev": 2, "doc": snap("Then")}).to_string(), &mut s);
        let (_, doc) = link.take_version_doc().unwrap();
        assert_eq!(super::version_doc(&doc.unwrap()).unwrap().plain_text(StoryRef::Body), "Then");
        assert!(super::version_doc("not json").is_none());
        assert!(link.restore(2));
        let out = super::edits_only(link.tick(&mut s, 0.0));
        let op = parse(&out[0]);
        assert_eq!((op["kind"].as_str(), op["cmd"].as_str(), op["params"]["version"].as_u64()), (Some("restore"), Some("collab.restore"), Some(2)));
        let (mut viewer, _) = joined("Now", Some("comment"));
        assert!(!viewer.restore(2), "only editors restore");
    }

    #[test]
    fn export_and_run_requests_go_to_the_host() {
        let (mut link, mut s) = joined("Hi", None);
        link.on_message(r#"{"t":"Export","id":"e1","format":"txt"}"#, &mut s);
        link.on_message(r#"{"t":"Run","id":"r1","cmd":"format.bold","params":{},"author":"API"}"#, &mut s);
        assert_eq!(
            link.take_host_requests(),
            [
                HostRequest::Export { id: "e1".into(), format: "txt".into() },
                HostRequest::Run { id: "r1".into(), cmd: "format.bold".into(), params: json!({}), author: "API".into() }
            ]
        );
        assert_eq!(parse(&CollabLink::exported("e1".into(), Ok(b"Hi".to_vec()))), json!({"t": "Exported", "id": "e1", "ok": true, "data": "SGk="}));
        assert_eq!(
            parse(&CollabLink::ran("r1".into(), Err("Unknown".into()))),
            json!({"t": "RunResult", "id": "r1", "ok": false, "error": "Unknown"})
        );
    }

    #[test]
    fn undo_goes_to_the_relay() {
        let (mut link, mut s) = joined("Hi", None);
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "!"})).unwrap();
        let out = super::edits_only(link.tick(&mut s, 0.0));
        let mut echo = parse(&out[0]);
        echo["rev"] = json!(4);
        link.on_message(&echo.to_string(), &mut s);
        assert!(link.can_undo());
        assert!(!link.can_redo());
        s.run("edit.undo", &json!({})).unwrap();
        let out = super::edits_only(link.tick(&mut s, 0.0));
        let op = parse(&out[0]);
        assert_eq!((op["kind"].as_str(), op["params"]["target"].as_u64()), (Some("undo"), Some(4)));
    }
}

#[cfg(test)]
mod offline_tests {
    use serde_json::json;
    use wordcraft_doc::Document;
    use wordcraft_engine::Session;

    use super::*;

    fn joined() -> (CollabLink, Session) {
        let mut src = Session::new(Document::new());
        src.run("text.insert", &json!({"text": "Hi"})).unwrap();
        let snap = String::from_utf8(wordcraft_engine::io::save_bytes("d.json", &src.doc).unwrap()).unwrap();
        let mut link = CollabLink::new(None);
        let mut s = Session::new(Document::new());
        link.on_message(&json!({"t": "Welcome", "rev": 1, "snapshot": snap, "ops": [], "you": "me"}).to_string(), &mut s);
        (link, s)
    }

    #[test]
    fn five_minutes_offline_is_read_only_until_back_and_synced() {
        let (mut link, mut s) = joined();
        link.tick(&mut s, 1000.0);
        link.on_message(r#"{"t":"Connection","online":false}"#, &mut s);
        link.tick(&mut s, 1000.0 + OFFLINE_LIMIT_MS - 1.0);
        assert!(!link.offline_locked());
        assert!(s.run("text.insert", &json!({"text": "!"})).is_ok(), "editing offline is fine for a while");
        let out = super::edits_only(link.tick(&mut s, 1000.0 + OFFLINE_LIMIT_MS));
        assert!(link.offline_locked());
        assert!(!link.online());
        assert!(s.run("text.insert", &json!({"text": "?"})).is_err());
        assert_eq!(link.take_note().as_deref(), Some("Reconnecting…"));
        link.on_message(r#"{"t":"Connection","online":true}"#, &mut s);
        link.tick(&mut s, 1000.0 + OFFLINE_LIMIT_MS + 10.0);
        assert!(link.offline_locked(), "still an edit waiting");
        let mut echo: serde_json::Value = serde_json::from_str(&out[0]).unwrap();
        echo["rev"] = json!(2);
        link.on_message(&echo.to_string(), &mut s);
        link.tick(&mut s, 1000.0 + OFFLINE_LIMIT_MS + 20.0);
        assert!(!link.offline_locked());
        assert!(s.run("text.insert", &json!({"text": "."})).is_ok());
    }

    #[test]
    fn a_thousand_edits_waiting_is_read_only_too() {
        let (mut link, mut s) = joined();
        s.run("caret.docEnd", &json!({})).unwrap();
        for _ in 0..PENDING_LIMIT {
            s.run("text.insert", &json!({"text": " "})).unwrap();
        }
        link.tick(&mut s, 0.0);
        assert!(link.offline_locked());
    }
}
