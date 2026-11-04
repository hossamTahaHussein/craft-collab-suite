//! Replica ↔ relay messages (serde JSON), the same for every app: the selection, the edit
//! context and a revision's recorded changes are the app's own types ([`CollabDoc`]).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::doc::CollabDoc;

/// Most bytes one message may have, except those that carry a whole document.
pub const MAX_OP_BYTES: usize = 256 * 1024;
/// Most bytes a message carrying a whole document may have.
pub const MAX_SNAPSHOT_BYTES: usize = 50 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OpKind {
    #[default]
    Cmd,
    Undo,
    Redo,
    Restore,
}

/// One edit: a command, the selection it ran at and the context it ran under.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", bound = "")]
pub struct Op<D: CollabDoc> {
    /// The replica that made it.
    pub client: String,
    /// Its number among that replica's edits.
    pub seq: u64,
    /// The last relay revision its replica had applied.
    pub base: u64,
    pub kind: OpKind,
    pub cmd: String,
    #[serde(default)]
    pub params: Value,
    pub sel: D::Sel,
    pub ctx: D::Ctx,
    /// Set by the relay: the edit's place in the document's history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<u64>,
    /// Set by the relay: who made it (the join token's name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// One revision's changes: what a replica needs to place an edit based on an earlier revision.
/// Snapshots carry the recent ones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", bound = "")]
pub struct HistEntry<D: CollabDoc> {
    pub rev: u64,
    #[serde(flatten)]
    pub change: D::Wire,
}

/// Someone else in the document: who, their colour (an index into the palette) and, once they
/// sent one, where their selection is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", bound = "")]
pub struct Peer<D: CollabDoc> {
    pub client: String,
    pub user: String,
    pub color: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sel: Option<D::Sel>,
    /// The confirmed revision `sel` is relative to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<u64>,
}

/// Relay → replica.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", bound = "")]
pub enum Inbound<D: CollabDoc> {
    /// After joining: the document at `rev` (a snapshot, or none when this is the first person
    /// and their editor provides the document), the edits after it, and this replica's client id.
    Welcome {
        rev: u64,
        snapshot: Option<String>,
        ops: Vec<Op<D>>,
        you: String,
        /// With a snapshot: the last of this replica's edits it holds (`Replica::resume`).
        #[serde(default)]
        seen: Option<u64>,
        /// With a snapshot: the changes of the revisions before it.
        #[serde(default)]
        history: Vec<HistEntry<D>>,
        /// The people already here (one that can't be read is left out, not the whole message).
        #[serde(default, deserialize_with = "readable_peers")]
        peers: Vec<Peer<D>>,
        /// The join token's role: `edit`, `suggest`, `comment` or `view` (an older relay sends
        /// none: `edit`).
        #[serde(default)]
        role: Option<String>,
        /// The name the relay knows this person by (their join token's): it stamps their edits
        /// with it, so the editor makes them under it too (an older relay sends none).
        #[serde(default)]
        user: Option<String>,
    },
    Op(Box<Op<D>>),
    /// Upload a snapshot of the confirmed document.
    SnapshotRequest,
    /// Start again from this snapshot and the edits after it.
    Resync {
        rev: u64,
        snapshot: String,
        ops: Vec<Op<D>>,
        #[serde(default)]
        seen: Option<u64>,
        #[serde(default)]
        history: Vec<HistEntry<D>>,
    },
    /// Save the document now (autosave).
    Save,
    /// Someone joined.
    Join(Peer<D>),
    /// Someone left.
    Leave {
        client: String,
    },
    /// Someone's selection moved.
    Presence(Peer<D>),
    /// The relay didn't take this replica's edit `seq` (its role doesn't allow it, or an undo
    /// names someone else's edit); it was not sequenced.
    Refused {
        seq: u64,
        reason: String,
    },
    /// Keep the confirmed document at `rev` as a version (answer with `Outbound::Version` only
    /// when the confirmed revision is `rev`).
    VersionRequest {
        rev: u64,
    },
    /// The document's versions, newest first.
    VersionList {
        versions: Vec<VersionInfo>,
    },
    /// One version's document, or none when it is gone.
    VersionDoc {
        rev: u64,
        doc: Option<String>,
    },
    /// The relay's HTTP API wants the document as a file of `format`.
    Export {
        id: String,
        format: String,
    },
    /// From the page, not the relay: whether it is connected to the relay now.
    Connection {
        online: bool,
    },
    /// The relay's HTTP API runs command `cmd` as an edit by `author`.
    Run {
        id: String,
        cmd: String,
        #[serde(default)]
        params: Value,
        author: String,
    },
}

/// One kept version: its revision, when it was kept (ISO time) and who edited since the one
/// before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub rev: u64,
    pub at: String,
    #[serde(default)]
    pub users: Vec<String>,
}

/// Replica → relay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", bound = "")]
pub enum Outbound<D: CollabDoc> {
    Op(Box<Op<D>>),
    /// The confirmed document's fingerprint at `rev` ([`CollabDoc::hash`]).
    Hash {
        rev: u64,
        h: String,
    },
    Snapshot {
        rev: u64,
        doc: String,
        #[serde(default)]
        history: Vec<HistEntry<D>>,
    },
    Saved {
        rev: u64,
    },
    /// This replica missed edits after `have`.
    ResyncRequest {
        have: u64,
        /// Start again from a snapshot the lease holder takes now, not the stored one (this
        /// replica can't apply an edit from what the stored one holds).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        fresh: bool,
    },
    /// Where this person's selection is, relative to the confirmed revision `rev` (sent at most
    /// ten times a second, and only with no edits unconfirmed; never logged).
    Presence {
        sel: D::Sel,
        rev: u64,
    },
    /// The confirmed document at `rev`, answering `VersionRequest`.
    Version {
        rev: u64,
        doc: String,
    },
    /// List the versions (`VersionList`).
    Versions,
    /// One version's document (`VersionDoc`).
    GetVersion {
        rev: u64,
    },
    /// Answers `Export`: the file's bytes in base64, or why there are none.
    Exported {
        id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Answers `Run`.
    RunResult {
        id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

fn readable_peers<'de, D: CollabDoc, De: serde::Deserializer<'de>>(d: De) -> Result<Vec<Peer<D>>, De::Error> {
    let all = Vec::<Value>::deserialize(d)?;
    Ok(all.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("message too large: {size} bytes (limit {limit})")]
    TooLarge { size: usize, limit: usize },
    #[error("bad message: {0}")]
    Json(#[from] serde_json::Error),
}

/// Parse a relay message, refusing one over its size limit.
pub fn decode_inbound<D: CollabDoc>(json: &str) -> Result<Inbound<D>, ProtoError> {
    if json.len() > MAX_SNAPSHOT_BYTES {
        return Err(ProtoError::TooLarge { size: json.len(), limit: MAX_SNAPSHOT_BYTES });
    }
    let message: Inbound<D> = serde_json::from_str(json)?;
    let carries_snapshot = match &message {
        Inbound::Welcome { .. } | Inbound::Resync { .. } | Inbound::VersionDoc { .. } => true,
        Inbound::Op(op) => op.kind == OpKind::Restore,
        _ => false,
    };
    if !carries_snapshot && json.len() > MAX_OP_BYTES {
        return Err(ProtoError::TooLarge { size: json.len(), limit: MAX_OP_BYTES });
    }
    Ok(message)
}
