//! The messages between a replica and the relay (see the co-editing spec, §3 and §8). Each is a
//! JSON object whose `t` names its type; field names are camelCase. The host page carries them
//! as strings, so a replica only ever sees `decode_inbound` and `serde_json::to_string`.
//!
//! The messages themselves are the suite's (`craft_collab_core::proto`); Docs supplies what a
//! selection is ([`Anchor`]: story offsets), the edit context (`RunCtx`) and what a revision
//! changed ([`HistSpan`]s).

use serde::{Deserialize, Serialize};
use wordcraft_doc::StoryRef;

use crate::adapter::DocsDoc;

pub use craft_collab_core::proto::{MAX_OP_BYTES, MAX_SNAPSHOT_BYTES, OpKind, ProtoError, VersionInfo};

/// A selection as story offsets (`wordcraft_doc::offsets`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Anchor {
    pub story: StoryRef,
    pub anchor: usize,
    pub head: usize,
}

/// One edit: a command, the selection it ran at and the context it ran under.
pub type Op = craft_collab_core::proto::Op<DocsDoc>;
/// One revision's changes (`mapping::Span`s, per story): what a replica needs to place an edit
/// based on an earlier revision. Snapshots carry the recent ones.
pub type HistEntry = craft_collab_core::proto::HistEntry<DocsDoc>;
/// Someone else in the document.
pub type Peer = craft_collab_core::proto::Peer<DocsDoc>;
/// Relay → replica.
pub type Inbound = craft_collab_core::proto::Inbound<DocsDoc>;
/// Replica → relay.
pub type Outbound = craft_collab_core::proto::Outbound<DocsDoc>;

/// What one revision changed, as `HistEntry` carries it: `{"rev": …, "spans": [...]}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spans {
    pub spans: Vec<HistSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistSpan {
    pub story: StoryRef,
    pub start: usize,
    pub old_end: usize,
    pub new_end: usize,
}

/// Parse a relay message, refusing one over its size limit.
pub fn decode_inbound(json: &str) -> Result<Inbound, ProtoError> {
    craft_collab_core::proto::decode_inbound::<DocsDoc>(json)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wordcraft_doc::StoryRef;
    use wordcraft_engine::RunCtx;

    use super::*;

    fn op() -> Op {
        Op {
            client: "c1".into(),
            seq: 42,
            base: 1187,
            kind: OpKind::Cmd,
            cmd: "text.insert".into(),
            params: json!({"text": "a"}),
            sel: Anchor { story: StoryRef::Body, anchor: 5120, head: 5120 },
            ctx: RunCtx { author: "Jane Doe".into(), ..RunCtx::default() },
            rev: None,
            user: None,
        }
    }

    #[test]
    fn op_round_trips_json() {
        let text = serde_json::to_string(&Outbound::Op(Box::new(op()))).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["t"], "Op");
        assert_eq!(v["cmd"], "text.insert");
        assert_eq!(v["sel"]["anchor"], 5120);
        assert_eq!(v["ctx"]["author"], "Jane Doe");
        let back = decode_inbound(&text).unwrap();
        assert_eq!(back, Inbound::Op(Box::new(op())));
    }

    #[test]
    fn relay_stamped_op_keeps_rev_and_user() {
        let mut v = serde_json::to_value(Outbound::Op(Box::new(op()))).unwrap();
        v["rev"] = json!(1188);
        v["user"] = json!("Jane Doe");
        let Inbound::Op(o) = decode_inbound(&v.to_string()).unwrap() else { panic!("not an op") };
        assert_eq!((o.rev, o.user.as_deref()), (Some(1188), Some("Jane Doe")));
    }

    #[test]
    fn welcome_and_resync_decode() {
        let w = decode_inbound(r#"{"t":"Welcome","rev":3,"snapshot":null,"ops":[],"you":"c1"}"#).unwrap();
        assert_eq!(
            w,
            Inbound::Welcome {
                rev: 3,
                snapshot: None,
                ops: vec![],
                you: "c1".into(),
                seen: None,
                history: vec![],
                peers: vec![],
                role: None,
                user: None
            }
        );
        assert_eq!(decode_inbound(r#"{"t":"Save"}"#).unwrap(), Inbound::Save);
        assert_eq!(decode_inbound(r#"{"t":"SnapshotRequest"}"#).unwrap(), Inbound::SnapshotRequest);
        let r = decode_inbound(r#"{"t":"Resync","rev":9,"snapshot":"{}","ops":[]}"#).unwrap();
        assert!(matches!(r, Inbound::Resync { rev: 9, seen: None, .. }));
        let r = decode_inbound(r#"{"t":"Resync","rev":9,"snapshot":"{}","ops":[],"seen":4}"#).unwrap();
        assert!(matches!(r, Inbound::Resync { seen: Some(4), .. }));
    }

    #[test]
    fn outbound_messages_have_their_tags() {
        let t = |m: &Outbound| serde_json::to_value(m).unwrap()["t"].as_str().unwrap().to_string();
        assert_eq!(t(&Outbound::Hash { rev: 50, h: "ab".into() }), "Hash");
        assert_eq!(t(&Outbound::Snapshot { rev: 1, doc: "{}".into(), history: vec![] }), "Snapshot");
        assert_eq!(t(&Outbound::Saved { rev: 1 }), "Saved");
        assert_eq!(serde_json::to_value(Outbound::ResyncRequest { have: 7, fresh: false }).unwrap(), json!({"t": "ResyncRequest", "have": 7}));
    }

    #[test]
    fn presence_messages_decode() {
        let j = decode_inbound(r#"{"t":"Join","client":"b","user":"B","color":1}"#).unwrap();
        assert_eq!(j, Inbound::Join(Peer { client: "b".into(), user: "B".into(), color: 1, sel: None, rev: None }));
        assert_eq!(decode_inbound(r#"{"t":"Leave","client":"b"}"#).unwrap(), Inbound::Leave { client: "b".into() });
        let p = decode_inbound(r#"{"t":"Presence","client":"a","user":"A","color":0,"sel":{"story":"body","anchor":3,"head":5}}"#).unwrap();
        let Inbound::Presence(peer) = p else { panic!("not presence") };
        assert_eq!(peer.sel, Some(Anchor { story: StoryRef::Body, anchor: 3, head: 5 }));
        let w =
            decode_inbound(r#"{"t":"Welcome","rev":0,"snapshot":null,"ops":[],"you":"c","peers":[{"client":"a","user":"A","color":0}]}"#).unwrap();
        let Inbound::Welcome { peers, .. } = w else { panic!("not welcome") };
        assert_eq!(peers.len(), 1);
        assert_eq!(
            serde_json::to_value(Outbound::Presence { sel: Anchor { story: StoryRef::Body, anchor: 2, head: 2 }, rev: 7 }).unwrap(),
            json!({"t": "Presence", "sel": {"story": "body", "anchor": 2, "head": 2}, "rev": 7})
        );
    }

    #[test]
    fn oversized_op_rejected() {
        let mut o = op();
        o.params = json!({"text": "x".repeat(MAX_OP_BYTES)});
        let text = serde_json::to_string(&Outbound::Op(Box::new(o))).unwrap();
        assert!(matches!(decode_inbound(&text), Err(ProtoError::TooLarge { .. })));
    }

    #[test]
    fn large_snapshot_is_allowed_up_to_its_own_limit() {
        let doc = "x".repeat(MAX_OP_BYTES * 2);
        let text = serde_json::json!({"t": "Resync", "rev": 1, "snapshot": doc, "ops": []}).to_string();
        assert!(decode_inbound(&text).is_ok());
    }

    #[test]
    fn protocol_v2_inbound_messages_decode() {
        let w = decode_inbound(r#"{"t":"Welcome","rev":0,"snapshot":null,"ops":[],"you":"c","role":"view"}"#).unwrap();
        assert!(matches!(w, Inbound::Welcome { role: Some(ref r), .. } if r == "view"));
        assert_eq!(
            decode_inbound(r#"{"t":"Refused","seq":4,"reason":"You can only view this document"}"#).unwrap(),
            Inbound::Refused { seq: 4, reason: "You can only view this document".into() }
        );
        assert_eq!(decode_inbound(r#"{"t":"VersionRequest","rev":12}"#).unwrap(), Inbound::VersionRequest { rev: 12 });
        assert_eq!(decode_inbound(r#"{"t":"Connection","online":false}"#).unwrap(), Inbound::Connection { online: false });
        assert_eq!(
            decode_inbound(r#"{"t":"VersionList","versions":[{"rev":9,"at":"2026-10-29T10:00:00Z","users":["Ann"]}]}"#).unwrap(),
            Inbound::VersionList { versions: vec![VersionInfo { rev: 9, at: "2026-10-29T10:00:00Z".into(), users: vec!["Ann".into()] }] }
        );
        assert_eq!(decode_inbound(r#"{"t":"VersionDoc","rev":9,"doc":null}"#).unwrap(), Inbound::VersionDoc { rev: 9, doc: None });
        assert_eq!(decode_inbound(r#"{"t":"Export","id":"e1","format":"pdf"}"#).unwrap(), Inbound::Export { id: "e1".into(), format: "pdf".into() });
        assert_eq!(
            decode_inbound(r#"{"t":"Run","id":"r1","cmd":"format.bold","params":{"value":true},"author":"API"}"#).unwrap(),
            Inbound::Run { id: "r1".into(), cmd: "format.bold".into(), params: json!({"value": true}), author: "API".into() }
        );
    }

    #[test]
    fn protocol_v2_outbound_messages_encode() {
        let v = |m: Outbound| serde_json::to_value(m).unwrap();
        assert_eq!(v(Outbound::Version { rev: 3, doc: "{}".into() }), json!({"t": "Version", "rev": 3, "doc": "{}"}));
        assert_eq!(v(Outbound::Versions), json!({"t": "Versions"}));
        assert_eq!(v(Outbound::GetVersion { rev: 3 }), json!({"t": "GetVersion", "rev": 3}));
        assert_eq!(
            v(Outbound::Exported { id: "e".into(), ok: true, data: Some("AA==".into()), error: None }),
            json!({"t": "Exported", "id": "e", "ok": true, "data": "AA=="})
        );
        assert_eq!(
            v(Outbound::RunResult { id: "r".into(), ok: false, result: None, error: Some("Unknown command".into()) }),
            json!({"t": "RunResult", "id": "r", "ok": false, "error": "Unknown command"})
        );
    }

    #[test]
    fn a_restore_op_or_version_may_be_as_large_as_a_snapshot() {
        let mut o = op();
        o.kind = OpKind::Restore;
        o.rev = Some(1);
        o.params = json!({"version": 3, "doc": "x".repeat(MAX_OP_BYTES + 10)});
        assert!(decode_inbound(&serde_json::to_string(&Outbound::Op(Box::new(o))).unwrap()).is_ok());
        let doc = json!({"t": "VersionDoc", "rev": 1, "doc": "x".repeat(MAX_OP_BYTES + 10)}).to_string();
        assert!(decode_inbound(&doc).is_ok());
    }

    /// The relay stores snapshots and their history as they are, so what a replica writes now
    /// must read back in any later one (and what older ones wrote, here).
    #[test]
    fn snapshot_and_history_keep_their_wire_shape() {
        let history = vec![HistEntry { rev: 1, change: Spans { spans: vec![HistSpan { story: StoryRef::Body, start: 0, old_end: 0, new_end: 1 }] } }];
        let snap = Outbound::Snapshot { rev: 1, doc: "{}".into(), history };
        let want = json!({"t": "Snapshot", "rev": 1, "doc": "{}", "history": [{"rev": 1, "spans": [{"story": "body", "start": 0, "oldEnd": 0, "newEnd": 1}]}]});
        assert_eq!(serde_json::to_value(&snap).unwrap(), want);
        let back: Outbound = serde_json::from_value(want).unwrap();
        assert_eq!(back, snap);
    }

    #[test]
    fn unknown_type_is_error() {
        assert!(matches!(decode_inbound(r#"{"t":"Nope"}"#), Err(ProtoError::Json(_))));
        assert!(matches!(decode_inbound("not json"), Err(ProtoError::Json(_))));
    }
}
