//! Co-editing glue between a page and a replica, the same for every app of the suite. The page
//! relays messages between the editor and the relay as JSON strings; this turns them into replica
//! calls on the visible session and returns what to send back: joining, catching up after a
//! reconnect or a resync, the person's edits one at a time, snapshots and fingerprints, where
//! everyone is (and where this person is, at most ten times a second), refused edits, the
//! offline limits, versions and what the relay's HTTP API asks of this editor. No browser APIs
//! here, so it is tested natively. An app plugs in with [`LinkHost`].
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod batch;

use std::collections::BTreeMap;
use std::fmt::Debug;

use craft_collab_core::doc::CollabDoc;
use craft_collab_core::proto::{HistEntry, Inbound, MAX_SNAPSHOT_BYTES, Op, Outbound, Peer, VersionInfo, decode_inbound};
use craft_collab_core::replica::{Applied, CollabError, Moved, Replica};
use serde_json::Value;

/// What the person may do in a co-edited document (their join token's role).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    #[default]
    Edit,
    /// Everything, but as tracked suggestions others accept or reject.
    Suggest,
    /// Comments only.
    Comment,
    /// Read only.
    View,
}

impl Role {
    /// The relay's name for a role (`edit`, `suggest`, `comment`, `view`); anything else is `edit`.
    pub fn parse(s: &str) -> Role {
        match s {
            "suggest" => Role::Suggest,
            "comment" => Role::Comment,
            "view" => Role::View,
            _ => Role::Edit,
        }
    }
}

/// What the link needs from an app beyond what the replica does.
pub trait LinkHost: CollabDoc {
    /// A position as the app draws it in the visible document.
    type Shown: Clone + Debug + PartialEq;
    /// A document from a snapshot or a kept version.
    fn decode(json: &str) -> Option<Self::Doc>;
    /// Which document the session shows (it changes when the host opens another).
    fn document_id(s: &Self::Session) -> u64;
    /// The document the first person opened, which becomes revision 0.
    fn opened(s: &Self::Session) -> Self::Doc;
    /// Show the shared document (named `name` for saving), unchanged since it was opened.
    fn show_shared(s: &mut Self::Session, doc: Self::Doc, name: Option<&str>);
    /// What the person may do (the relay refuses the rest anyway; this says so first).
    fn set_role(s: &mut Self::Session, role: Role);
    /// Viewing and its lock, as they are.
    fn read_only(s: &Self::Session) -> (bool, bool);
    fn set_read_only(s: &mut Self::Session, viewing: (bool, bool));
    /// A number that changes whenever the visible document does.
    fn visible_rev(s: &Self::Session) -> u64;
    fn set_collab(s: &mut Self::Session, on: bool);
    /// `sel` (in the visible document's terms) as the app draws it; `None` when it is gone.
    fn show(s: &Self::Session, sel: &Self::Sel) -> Option<Self::Shown>;
}

/// How often a replica may say where its selection is.
const PRESENCE_EVERY_MS: f64 = 100.0;
/// How long a peer counts as typing after its last edit.
const TYPING_MS: f64 = 3000.0;
/// Offline this long, or this many edits waiting, and the document goes read only until they
/// sync (spec §7): past that, a person risks losing much more than they would gain.
pub const OFFLINE_LIMIT_MS: f64 = 5.0 * 60.0 * 1000.0;
pub const PENDING_LIMIT: usize = 1000;

/// Someone else in the document, as the UI shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct PeerView<S> {
    pub client: String,
    pub name: String,
    pub color: u8,
    pub sel: Option<S>,
    pub typed_at: Option<f64>,
}

impl<S> PeerView<S> {
    /// Whether they edited in the last three seconds (their name flag shows).
    pub fn typing(&self, now_ms: f64) -> bool {
        self.typed_at.is_some_and(|t| now_ms - t < TYPING_MS)
    }
}

struct PeerState<H: LinkHost> {
    name: String,
    color: u8,
    /// Where they are, in the confirmed document's offsets at a revision (the replica's, unless
    /// a snapshot replaced it since).
    anchor: Option<(H::Sel, u64)>,
    /// Where they said they are, at a revision not placed yet.
    waiting: Option<(H::Sel, u64)>,
    typed_at: Option<f64>,
}

/// What the relay's HTTP API asked of this editor (the host answers with
/// [`CollabLink::exported`] and [`CollabLink::ran`]).
#[derive(Clone, Debug, PartialEq)]
pub enum HostRequest {
    Export { id: String, format: String },
    Run { id: String, cmd: String, params: Value, author: String },
}

pub struct CollabLink<H: LinkHost> {
    replica: Option<Replica<H>>,
    /// This replica's client id (from `Welcome`).
    you: Option<String>,
    /// The first person: waiting for the host to open the document, which was open as this id.
    awaiting: Option<u64>,
    /// The document's file name, for saving (`?docName=`).
    doc_name: Option<String>,
    document_ready: bool,
    save_requested: bool,
    /// A message for the person (an edit of theirs couldn't be applied).
    note: Option<String>,
    /// The others here, by client id.
    peers: BTreeMap<String, PeerState<H>>,
    /// The visible document's revision when it last matched the confirmed one (nothing of this
    /// person's unconfirmed).
    clean: Option<u64>,
    /// This replica's selection as last sent, and when.
    sent: Option<(H::Sel, f64)>,
    /// The selection moved but wasn't sent yet (too soon after the last one).
    presence_waiting: bool,
    /// The time of the last tick, for marking who typed.
    now: f64,
    /// The names of the people whose edits were applied since [`CollabLink::take_edited_by`].
    edited_by: Vec<String>,
    /// What the person may do (the join token's role).
    role: Role,
    host_requests: Vec<HostRequest>,
    /// The version list and a version's document, as the relay last sent them.
    versions: Option<Vec<VersionInfo>>,
    version_doc: Option<(u64, Option<String>)>,
    /// Since when the page has been disconnected from the relay.
    offline_since: Option<f64>,
    /// Read only for being offline too long: Viewing and its lock as they were before.
    offline_lock: Option<(bool, bool)>,
}

impl<H: LinkHost> CollabLink<H> {
    pub fn new(doc_name: Option<String>) -> CollabLink<H> {
        CollabLink {
            replica: None,
            you: None,
            awaiting: None,
            doc_name,
            document_ready: false,
            save_requested: false,
            note: None,
            peers: BTreeMap::new(),
            clean: None,
            sent: None,
            presence_waiting: false,
            now: 0.0,
            edited_by: Vec::new(),
            role: Role::Edit,
            host_requests: Vec::new(),
            versions: None,
            version_doc: None,
            offline_since: None,
            offline_lock: None,
        }
    }

    /// Whether the page is connected to the relay.
    pub fn online(&self) -> bool {
        self.offline_since.is_none()
    }

    /// The document is read only until this person's edits sync (offline too long, or too many
    /// waiting).
    pub fn offline_locked(&self) -> bool {
        self.offline_lock.is_some()
    }

    /// Go read only past the offline limits; back to editing once online with nothing waiting.
    fn check_offline(&mut self, visible: &mut H::Session) {
        let over = self.offline_since.is_some_and(|t| self.now - t >= OFFLINE_LIMIT_MS) || self.pending() >= PENDING_LIMIT;
        if over && self.offline_lock.is_none() {
            self.offline_lock = Some(H::read_only(visible));
            H::set_read_only(visible, (true, true));
            self.note = Some("Reconnecting…".into());
        } else if self.offline_since.is_none()
            && self.pending() == 0
            && let Some(was) = self.offline_lock.take()
        {
            H::set_read_only(visible, was);
        }
    }

    /// The relay's HTTP API requests since the last call.
    pub fn take_host_requests(&mut self) -> Vec<HostRequest> {
        std::mem::take(&mut self.host_requests)
    }

    /// Answer an `Export`: the file's bytes, or why there are none.
    pub fn exported(id: String, file: Result<Vec<u8>, String>) -> String {
        use base64::Engine as _;
        // Base64 takes 4 bytes for 3; the message must stay under the relay's limit.
        let fits = |len: usize| len.div_ceil(3) * 4 + 1024 <= MAX_SNAPSHOT_BYTES;
        encode::<H>(&match file {
            Ok(bytes) if !fits(bytes.len()) => Outbound::Exported {
                id,
                ok: false,
                data: None,
                error: Some(format!("The file is too large to send ({} MB)", bytes.len() / (1024 * 1024))),
            },
            Ok(bytes) => Outbound::Exported { id, ok: true, data: Some(base64::engine::general_purpose::STANDARD.encode(bytes)), error: None },
            Err(e) => Outbound::Exported { id, ok: false, data: None, error: Some(e) },
        })
    }

    /// Answer a `Run`.
    pub fn ran(id: String, result: Result<Value, String>) -> String {
        encode::<H>(&match result {
            Ok(v) => Outbound::RunResult { id, ok: true, result: Some(v), error: None },
            Err(e) => Outbound::RunResult { id, ok: false, result: None, error: Some(e) },
        })
    }

    /// Ask for the version list (it arrives in [`CollabLink::take_versions`]).
    pub fn list_versions(&self) -> String {
        encode::<H>(&Outbound::Versions)
    }

    /// Ask for one version's document (it arrives in [`CollabLink::take_version_doc`]).
    pub fn get_version(&self, rev: u64) -> String {
        encode::<H>(&Outbound::GetVersion { rev })
    }

    pub fn take_versions(&mut self) -> Option<Vec<VersionInfo>> {
        self.versions.take()
    }

    pub fn take_version_doc(&mut self) -> Option<(u64, Option<String>)> {
        self.version_doc.take()
    }

    /// Restore the version at `rev` for everyone (editors only; it goes out with the next tick).
    pub fn restore(&mut self, rev: u64) -> bool {
        if self.role != Role::Edit {
            return false;
        }
        self.replica.as_mut().map(|r| r.restore(rev)).is_some()
    }

    pub fn can_undo(&self) -> bool {
        self.replica.as_ref().is_some_and(Replica::<H>::can_undo)
    }

    pub fn can_redo(&self) -> bool {
        self.replica.as_ref().is_some_and(Replica::<H>::can_redo)
    }

    pub fn rev(&self) -> Option<u64> {
        self.replica.as_ref().map(Replica::<H>::rev)
    }

    pub fn pending(&self) -> usize {
        self.replica.as_ref().map_or(0, Replica::<H>::pending_len)
    }

    /// True once, when the shared document is first shown (the host leaves the start screen).
    pub fn take_document_ready(&mut self) -> bool {
        std::mem::take(&mut self.document_ready)
    }

    /// A message for the status bar, once.
    pub fn take_note(&mut self) -> Option<String> {
        self.note.take()
    }

    /// True once after the relay asked this replica to save.
    pub fn take_save_request(&mut self) -> bool {
        std::mem::take(&mut self.save_requested)
    }

    /// The `Saved` message for the confirmed revision (after the host saved).
    pub fn saved(&self) -> String {
        encode::<H>(&Outbound::Saved { rev: self.rev().unwrap_or(0) })
    }

    /// Handle one relay message; returns the messages to send back. Bad messages are logged and
    /// ignored.
    pub fn on_message(&mut self, json: &str, visible: &mut H::Session) -> Vec<String> {
        let message = match decode_inbound::<H>(json) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("co-editing: ignored a relay message: {e}");
                // An edit nobody can read still takes its revision: skip it, as every replica does.
                if let Some(rev) = unreadable_op_rev(json)
                    && let Some(replica) = self.replica.as_mut()
                    && rev == replica.rev() + 1
                    && replica.skip(rev).is_ok()
                {
                    return Vec::new();
                }
                return Vec::new();
            }
        };
        let out = self.handle(message, visible);
        self.follow(visible);
        out
    }

    fn handle(&mut self, message: Inbound<H>, visible: &mut H::Session) -> Vec<String> {
        match message {
            Inbound::Welcome { rev, snapshot, ops, you, seen, history, peers, role } => {
                self.role = Role::parse(role.as_deref().unwrap_or("edit"));
                H::set_role(visible, self.role);
                // The relay's list replaces ours (a reconnect may have missed joins and leaves).
                self.peers.clear();
                self.sent = None;
                let welcome = self.welcome(rev, snapshot, ops, you, seen, history, visible);
                self.follow(visible);
                for peer in peers {
                    self.note_peer(peer);
                }
                welcome
            }
            Inbound::Op(op) => self.apply_op(*op, visible),
            Inbound::SnapshotRequest => match self.replica.as_ref().map(Replica::<H>::snapshot) {
                Some(Ok(s)) => vec![encode::<H>(&s)],
                Some(Err(e)) => {
                    log::warn!("co-editing: no snapshot: {e}");
                    Vec::new()
                }
                None => Vec::new(),
            },
            Inbound::Resync { rev, snapshot, ops, seen, history } => {
                if self.replica.is_none() {
                    // It couldn't join from its Welcome: start from this snapshot instead.
                    let you = self.you.clone().unwrap_or_default();
                    return self.join(rev, snapshot, history, ops, you, visible);
                }
                self.resume(rev, Some(snapshot), ops, seen, history, visible)
            }
            Inbound::Save => {
                self.save_requested = self.replica.is_some();
                Vec::new()
            }
            Inbound::Join(peer) | Inbound::Presence(peer) => {
                self.note_peer(peer);
                Vec::new()
            }
            Inbound::Leave { client } => {
                self.peers.remove(&client);
                Vec::new()
            }
            Inbound::Refused { seq, reason } => {
                if let Some(replica) = self.replica.as_mut() {
                    replica.refused(seq, visible);
                }
                self.note = Some(reason);
                Vec::new()
            }
            // Only the confirmed document at exactly that revision is a version of it.
            // The document as confirmed now: others may have edited since the save it is for.
            Inbound::VersionRequest { rev } => match self.replica.as_ref().filter(|r| r.rev() >= rev).map(Replica::<H>::snapshot) {
                Some(Ok(Outbound::Snapshot { rev, doc, .. })) => vec![encode::<H>(&Outbound::Version { rev, doc })],
                _ => Vec::new(),
            },
            Inbound::VersionList { versions } => {
                self.versions = Some(versions);
                Vec::new()
            }
            Inbound::VersionDoc { rev, doc } => {
                self.version_doc = Some((rev, doc));
                Vec::new()
            }
            Inbound::Export { id, format } => {
                self.host_requests.push(HostRequest::Export { id, format });
                Vec::new()
            }
            Inbound::Run { id, cmd, params, author } => {
                self.host_requests.push(HostRequest::Run { id, cmd, params, author });
                Vec::new()
            }
            Inbound::Connection { online } => {
                self.offline_since = if online { None } else { self.offline_since.or(Some(self.now)) };
                Vec::new()
            }
        }
    }

    /// After the person's edits this frame: the next edit to send and a fingerprint when due;
    /// for the first person, the opened document as revision 0.
    pub fn tick(&mut self, visible: &mut H::Session, now_ms: f64) -> Vec<String> {
        self.now = now_ms;
        if let Some(blank) = self.awaiting {
            if H::document_id(visible) == blank {
                return Vec::new();
            }
            self.awaiting = None;
            let you = self.you.clone().unwrap_or_default();
            match Replica::<H>::join(you, 0, H::opened(visible), Vec::new()) {
                Ok(r) => {
                    H::set_collab(visible, true);
                    self.document_ready = true;
                    let snapshot = r.snapshot();
                    self.replica = Some(r);
                    self.follow(visible);
                    return snapshot.map(|s| vec![encode::<H>(&s)]).unwrap_or_default();
                }
                Err(e) => log::warn!("co-editing: couldn't start: {e}"),
            }
        }
        let Some(replica) = self.replica.as_mut() else { return Vec::new() };
        replica.drain_local(visible);
        self.follow(visible);
        self.check_offline(visible);
        let Some(replica) = self.replica.as_mut() else { return Vec::new() };
        let mut out = Vec::new();
        if let Some(op) = replica.next_to_send() {
            out.push(encode::<H>(&Outbound::Op(Box::new(op))));
        }
        if let Some(note) = replica.take_notes().pop() {
            self.note = Some(note);
        }
        if replica.take_needs_fresh() {
            out.push(encode::<H>(&Outbound::ResyncRequest { have: replica.rev(), fresh: true }));
        }
        if let Some(h) = replica.hash_due() {
            out.push(encode::<H>(&h));
        }
        if let Some(p) = self.presence_due(visible, now_ms) {
            out.push(p);
        }
        out
    }

    /// The others here, their selections in the visible document.
    pub fn peers(&self, visible: &H::Session) -> Vec<PeerView<H::Shown>> {
        let replica = self.replica.as_ref();
        // This person's unconfirmed edits, which move everyone on their screen (worked out only
        // when someone has a position and there are such edits).
        let mut mine = None;
        let mut on_screen = |a: &H::Sel| {
            let replica = replica?;
            let mine = mine.get_or_insert_with(|| {
                if self.clean == Some(H::visible_rev(visible)) {
                    H::Change::default()
                } else {
                    H::changes(replica.confirmed(), H::doc(visible), None)
                }
            });
            H::show(visible, &H::map(a, mine))
        };
        self.peers
            .iter()
            .map(|(client, p)| PeerView {
                client: client.clone(),
                name: p.name.clone(),
                color: p.color,
                sel: p.anchor.as_ref().and_then(|(a, _)| on_screen(a)),
                typed_at: p.typed_at,
            })
            .collect()
    }

    /// The names of the people whose edits were applied since the last call, in order.
    pub fn take_edited_by(&mut self) -> Vec<String> {
        std::mem::take(&mut self.edited_by)
    }

    /// The selection moved but wasn't sent yet: the host should tick again shortly, even
    /// without input.
    pub fn presence_pending(&self) -> bool {
        self.presence_waiting
    }

    fn note_peer(&mut self, peer: Peer<H>) {
        let entry =
            self.peers.entry(peer.client.clone()).or_insert(PeerState { name: String::new(), color: 0, anchor: None, waiting: None, typed_at: None });
        entry.name = peer.user;
        entry.color = peer.color;
        // A position without its revision can't be placed.
        if let (Some(sel), Some(rev)) = (peer.sel, peer.rev) {
            entry.waiting = Some((sel, rev));
        }
        self.place_waiting();
    }

    /// Move everyone through the edits the replica confirmed since the last call, each through
    /// that edit's own change; the edit's author goes where it left them. Then place the
    /// positions people sent that this replica can now place.
    fn follow(&mut self, visible: &H::Session) {
        let Some(replica) = self.replica.as_mut() else { return };
        let restarted = replica.take_restarted();
        let moved = replica.take_moved();
        self.clean = (replica.pending_len() == 0).then(|| H::visible_rev(visible));
        if restarted {
            // The offsets were the old confirmed document's: place them again from their revision.
            for p in self.peers.values_mut() {
                if let Some((a, at)) = p.anchor.take()
                    && p.waiting.as_ref().is_none_or(|(_, w)| *w < at)
                {
                    p.waiting = Some((a, at));
                }
            }
        }
        let me = self.you.clone().unwrap_or_default();
        for m in moved {
            if let Some(p) = self.peers.get(&m.client).filter(|_| m.client != me) {
                self.edited_by.push(p.name.clone());
            }
            if m.client == me {
                // Others saw this person's caret move with the edit: say again where it is.
                self.sent = None;
            }
            for (client, p) in self.peers.iter_mut() {
                follow_one::<H>(client, p, &m, self.now);
            }
        }
        self.place_waiting();
    }

    /// Place the positions people sent, once this replica has the revision each is relative to.
    fn place_waiting(&mut self) {
        let Some(replica) = self.replica.as_ref() else { return };
        for p in self.peers.values_mut() {
            let Some((sel, rev)) = p.waiting.clone() else { continue };
            if rev > replica.rev() {
                continue;
            }
            p.waiting = None;
            p.anchor = replica.rebase_anchor(&sel, rev).map(|a| (a, replica.rev()));
        }
    }

    /// This person's selection for the relay, when it moved and the last one is 100 ms old.
    fn presence_due(&mut self, visible: &H::Session, now_ms: f64) -> Option<String> {
        self.presence_waiting = false;
        // With edits unconfirmed the visible offsets aren't the confirmed revision's: wait (their
        // confirmation brings a message, and a tick).
        let replica = self.replica.as_ref()?;
        if replica.pending_len() > 0 {
            return None;
        }
        let rev = replica.rev();
        let here = H::selection(visible)?;
        if self.sent.as_ref().is_some_and(|(last, _)| *last == here) {
            return None;
        }
        if self.sent.as_ref().is_some_and(|(_, at)| now_ms - at < PRESENCE_EVERY_MS) {
            self.presence_waiting = true;
            return None;
        }
        self.sent = Some((here.clone(), now_ms));
        Some(encode::<H>(&Outbound::Presence { sel: here, rev }))
    }

    #[allow(clippy::too_many_arguments)]
    fn welcome(
        &mut self,
        rev: u64,
        snapshot: Option<String>,
        ops: Vec<Op<H>>,
        you: String,
        seen: Option<u64>,
        history: Vec<HistEntry<H>>,
        visible: &mut H::Session,
    ) -> Vec<String> {
        self.you = Some(you.clone());
        if self.replica.is_some() {
            // A reconnect: catch up and send what the relay didn't get.
            return self.resume(rev, snapshot, ops, seen, history, visible);
        }
        let Some(json) = snapshot else {
            // The first person: the host opens the document, which becomes revision 0.
            self.awaiting = Some(H::document_id(visible));
            return Vec::new();
        };
        self.join(rev, json, history, ops, you, visible)
    }

    /// Start the replica from a snapshot and show it; on failure ask for a resync.
    fn join(&mut self, rev: u64, json: String, history: Vec<HistEntry<H>>, ops: Vec<Op<H>>, you: String, visible: &mut H::Session) -> Vec<String> {
        let Some(doc) = H::decode(&json) else {
            log::warn!("co-editing: bad snapshot");
            return vec![encode::<H>(&Outbound::ResyncRequest { have: rev, fresh: false })];
        };
        match Replica::<H>::join_with_history(you, rev, doc, history, ops) {
            Ok(r) => {
                H::show_shared(visible, r.confirmed().clone(), self.doc_name.as_deref());
                r.attach(visible);
                self.replica = Some(r);
                self.document_ready = true;
                Vec::new()
            }
            Err(e) => {
                log::warn!("co-editing: couldn't join: {e}");
                vec![encode::<H>(&Outbound::ResyncRequest { have: rev, fresh: false })]
            }
        }
    }

    fn apply_op(&mut self, op: Op<H>, visible: &mut H::Session) -> Vec<String> {
        let Some(replica) = self.replica.as_mut() else { return Vec::new() };
        match replica.receive(op, visible) {
            Ok(Applied::Remote { dropped_pending }) if dropped_pending > 0 => {
                self.note = Some(dropped_note(dropped_pending));
                Vec::new()
            }
            Ok(_) => Vec::new(),
            Err(CollabError::Gap { have, .. }) | Err(CollabError::TooOld { oldest: have, .. }) => {
                vec![encode::<H>(&Outbound::ResyncRequest { have, fresh: false })]
            }
            Err(e) => {
                log::warn!("co-editing: {e}");
                vec![encode::<H>(&Outbound::ResyncRequest { have: replica.rev(), fresh: false })]
            }
        }
    }

    fn resume(
        &mut self,
        rev: u64,
        snapshot: Option<String>,
        ops: Vec<Op<H>>,
        seen: Option<u64>,
        history: Vec<HistEntry<H>>,
        visible: &mut H::Session,
    ) -> Vec<String> {
        let Some(replica) = self.replica.as_mut() else { return Vec::new() };
        let doc = match snapshot.map(|json| H::decode(&json)) {
            None => None,
            Some(Some(d)) => Some(d),
            Some(None) => {
                log::warn!("co-editing: bad snapshot");
                return Vec::new();
            }
        };
        match replica.resume(rev, doc.map(|d| (d, history)), ops, seen, visible) {
            Ok(resend) => resend.into_iter().map(|op| encode::<H>(&Outbound::Op(Box::new(op)))).collect(),
            Err(e) => {
                log::warn!("co-editing: couldn't catch up: {e}");
                vec![encode::<H>(&Outbound::ResyncRequest { have: replica.rev(), fresh: false })]
            }
        }
    }
}

/// One peer through one confirmed edit `m`.
fn follow_one<H: LinkHost>(client: &str, p: &mut PeerState<H>, m: &Moved<H>, now: f64) {
    if client == m.client {
        // Unless they said where they are after this edit, they are where it left them.
        if p.waiting.as_ref().is_none_or(|(_, w)| *w < m.rev) && p.anchor.as_ref().is_none_or(|(_, at)| *at < m.rev) {
            p.waiting = None;
            if let Some(a) = &m.author_sel {
                p.anchor = Some((a.clone(), m.rev));
            }
            p.typed_at = Some(now);
        }
    }
    if let Some((a, at)) = p.anchor.as_mut()
        && *at + 1 == m.rev
    {
        *a = H::map(a, &m.spans);
        *at = m.rev;
    }
}

fn dropped_note(n: usize) -> String {
    if n == 1 {
        "Your last edit couldn't be applied after someone else's change.".to_string()
    } else {
        format!("{n} of your edits couldn't be applied after someone else's change.")
    }
}

/// The revision of a relay op that couldn't be decoded, if it has one.
fn unreadable_op_rev(json: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    (v.get("t")?.as_str()? == "Op").then(|| v.get("rev")?.as_u64())?
}

fn encode<H: LinkHost>(m: &Outbound<H>) -> String {
    serde_json::to_string(m).unwrap_or_default()
}
