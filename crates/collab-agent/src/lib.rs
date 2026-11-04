//! Agents join a co-edited document of the suite (`collab-mcp`): an MCP server whose session is a
//! participant in a shared document, as Craft Docs' `wordcraft-cli collab-mcp` is. It runs the
//! same engine as everyone's editor (a headless replica), so its edits sync live under its name,
//! its selection shows, and people see it work. It never holds the autosave lease and is never
//! asked for snapshots, versions or exports (`hello` carries `agent: true`).
//!
//! The app's MCP backend owns the session; [`Agent`] keeps the replica and the relay connection.
//! Each tool call first takes in what the relay sent ([`Agent::before`]), runs on the shared
//! document, then sends the agent's edits and waits (up to [`CONFIRM_WAIT`]) for the relay to
//! confirm them ([`Agent::after`]), so a reply describes the document as everyone has it.

use std::collections::BTreeMap;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use craft_collab_core::proto::{Inbound, Op, Outbound, decode_inbound};
use craft_collab_core::replica::{CollabError, Replica};
use craft_collab_link::{LinkHost, Role};
use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// How long joining waits for the relay's `Welcome`.
const JOIN_WAIT: Duration = Duration::from_secs(15);
/// How long a call waits for the relay to confirm the agent's edits.
pub const CONFIRM_WAIT: Duration = Duration::from_secs(10);
/// The most `collab.wait` waits.
const MAX_WAIT: Duration = Duration::from_secs(120);
/// Changes kept for `collab.wait`.
const MAX_CHANGES: usize = 2000;

/// What an agent needs from an app besides its co-editing adapter `H`.
pub struct App<H: LinkHost> {
    /// The app as the relay names it: 'docs', 'sheets' or 'slides'.
    pub id: &'static str,
    /// The editor's name in messages ("Craft Sheets").
    pub name: &'static str,
    /// Sign the session's edits and comments as this name.
    pub set_author: fn(&mut H::Session, &str),
}

/// A connection to the relay carrying its JSON messages.
pub trait Wire {
    fn send(&mut self, msg: &str) -> Result<(), String>;
    /// The next message, waiting up to `wait`; `Ok(None)` when none came in time.
    fn recv(&mut self, wait: Duration) -> Result<Option<String>, WireError>;
}

/// Why a connection to the relay ended.
#[derive(Debug)]
pub enum WireError {
    /// It broke: try again.
    Lost(String),
    /// The relay won't have the agent back (its token, wrong app, removed, the document closed).
    Ended(String),
}

impl WireError {
    /// The relay closed the connection with `code` and `reason`.
    pub fn closed(code: u16, reason: &str) -> WireError {
        let why =
            if reason.is_empty() { "the relay closed the connection".to_string() } else { format!("the relay closed the connection: {reason}") };
        // As the SDK: a bad client or token, the wrong app, removed or the document closed are final.
        if matches!(code, 4400 | 4401 | 4403 | 4410) { WireError::Ended(why) } else { WireError::Lost(why) }
    }

    fn message(self) -> String {
        match self {
            WireError::Lost(m) | WireError::Ended(m) => m,
        }
    }
}

/// The relay's WebSocket (`ws://` or `wss://`).
pub struct WsWire {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl WsWire {
    pub fn connect(url: &str) -> Result<WsWire, String> {
        let (ws, _) = tungstenite::connect(url).map_err(|e| format!("can't reach the relay at {url}: {e}"))?;
        Ok(WsWire { ws })
    }

    fn set_timeout(&mut self, wait: Duration) {
        let wait = Some(wait.max(Duration::from_millis(1)));
        let r = match self.ws.get_mut() {
            MaybeTlsStream::Plain(s) => s.set_read_timeout(wait),
            MaybeTlsStream::Rustls(s) => s.get_mut().set_read_timeout(wait),
            _ => Ok(()),
        };
        if let Err(e) = r {
            eprintln!("collab-mcp: {e}");
        }
    }
}

impl Wire for WsWire {
    fn send(&mut self, msg: &str) -> Result<(), String> {
        self.ws.send(Message::text(msg)).map_err(|e| format!("the relay connection failed: {e}"))
    }

    fn recv(&mut self, wait: Duration) -> Result<Option<String>, WireError> {
        self.set_timeout(wait);
        match self.ws.read() {
            Ok(Message::Text(t)) => Ok(Some(t.to_string())),
            Ok(Message::Close(frame)) => Err(match frame {
                Some(f) => WireError::closed(f.code.into(), &f.reason),
                None => WireError::closed(1005, ""),
            }),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(WireError::Lost(format!("the relay connection failed: {e}"))),
        }
    }
}

/// One confirmed edit, for `collab.wait`.
#[derive(Clone, Debug, PartialEq)]
struct Change {
    rev: u64,
    user: String,
    command: String,
}

/// An agent's place in a shared document: the replica and the relay connection. The session it
/// edits is the app's MCP backend's, passed to each call.
pub struct Agent<H: LinkHost> {
    wire: Box<dyn Wire>,
    /// Opens a new connection after the old one failed.
    reconnect: Box<dyn FnMut() -> Result<Box<dyn Wire>, String>>,
    token: String,
    client: String,
    name: String,
    replica: Replica<H>,
    app: App<H>,
    /// The others here: client id → (name, colour).
    peers: BTreeMap<String, (String, u8)>,
    changes: Vec<Change>,
    /// Why the relay refused an edit of this call's.
    refused: Option<String>,
    relay: String,
    /// The relay won't have the agent back: why (every later call fails with it).
    ended: Option<String>,
}

/// A client id for this connection.
fn client_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("agent{:x}{:x}", nanos, std::process::id())
}

/// The `hello` an agent of `app` sends.
fn hello(app: &str, token: &str, client: &str, have: Option<u64>) -> String {
    let mut m = json!({"t": "hello", "token": token, "client": client, "agent": true, "app": app});
    if let Some(rev) = have {
        m["haveRev"] = json!(rev);
    }
    m.to_string()
}

impl<H: LinkHost> Agent<H> {
    /// Join the relay at `relay` (a `ws://` or `wss://` URL) with join token `token`, as `name`,
    /// into `session` (the MCP backend's).
    pub fn connect(app: App<H>, relay: &str, token: &str, name: &str, session: &mut H::Session) -> Result<Agent<H>, String> {
        let url = relay.to_string();
        let wire = WsWire::connect(&url)?;
        let reconnect = Box::new(move || WsWire::connect(&url).map(|w| Box::new(w) as Box<dyn Wire>));
        let mut a = Agent::join(app, Box::new(wire), reconnect, token, name, session)?;
        a.relay = relay.to_string();
        Ok(a)
    }

    /// Join over `wire` (tests give an in-memory one).
    pub fn join(
        app: App<H>,
        mut wire: Box<dyn Wire>,
        reconnect: Box<dyn FnMut() -> Result<Box<dyn Wire>, String>>,
        token: &str,
        name: &str,
        session: &mut H::Session,
    ) -> Result<Agent<H>, String> {
        let client = client_id();
        wire.send(&hello(app.id, token, &client, None))?;
        let deadline = Instant::now() + JOIN_WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                // The relay holds an agent back until a person opens the document.
                return Err(
                    "no welcome from the relay in 15 s: nobody has this document open yet (an agent joins a document a person has open)".into()
                );
            }
            let Some(msg) = wire.recv(left).map_err(WireError::message)? else { continue };
            let Ok(Inbound::<H>::Welcome { rev, snapshot, ops, you, history, peers, role, .. }) = decode_inbound::<H>(&msg) else { continue };
            let Some(json) = snapshot else {
                return Err("nobody has opened this document yet: an agent joins a document a person has open".into());
            };
            let doc = H::decode(&json).ok_or_else(|| format!("the shared document isn't a {} document", app.name))?;
            let replica = Replica::join_with_history(you, rev, doc, history, ops).map_err(|e| e.to_string())?;
            (app.set_author)(session, name);
            replica.attach(session);
            H::set_role(session, Role::parse(role.as_deref().unwrap_or("edit")));
            let peers = peers.into_iter().map(|p| (p.client, (p.user, p.color))).collect();
            return Ok(Agent {
                wire,
                reconnect,
                token: token.to_string(),
                client,
                name: name.to_string(),
                replica,
                app,
                peers,
                changes: Vec::new(),
                refused: None,
                relay: String::new(),
                ended: None,
            });
        }
    }

    fn send(&mut self, m: &Outbound<H>) -> Result<(), String> {
        let text = serde_json::to_string(m).map_err(|e| e.to_string())?;
        self.wire.send(&text)
    }

    fn send_ops(&mut self, ops: Vec<Op<H>>) -> Result<(), String> {
        for op in ops {
            self.send(&Outbound::Op(Box::new(op)))?;
        }
        Ok(())
    }

    /// Take in one relay message.
    fn handle(&mut self, msg: &str, session: &mut H::Session) -> Result<(), String> {
        let message = match decode_inbound::<H>(msg) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("collab-mcp: ignored a relay message: {e}");
                return Ok(());
            }
        };
        match message {
            Inbound::Op(op) => {
                let (rev, user, command) = (op.rev.unwrap_or(0), op.user.clone().unwrap_or_default(), op.cmd.clone());
                match self.replica.receive(*op, session) {
                    Ok(_) => {
                        if self.replica.take_needs_fresh() {
                            self.send(&Outbound::ResyncRequest { have: self.replica.rev(), fresh: true })?;
                        }
                        self.changes.push(Change { rev, user, command });
                        if self.changes.len() > MAX_CHANGES {
                            self.changes.remove(0);
                        }
                    }
                    Err(CollabError::Gap { have, .. }) | Err(CollabError::TooOld { oldest: have, .. }) => {
                        self.send(&Outbound::ResyncRequest { have, fresh: false })?
                    }
                    Err(e) => return Err(e.to_string()),
                }
            }
            Inbound::Resync { rev, snapshot, ops, seen, history } => {
                let doc = H::decode(&snapshot).ok_or_else(|| format!("the shared document isn't a {} document", self.app.name))?;
                let ops = self.replica.resume(rev, Some((doc, history)), ops, seen, session).map_err(|e| e.to_string())?;
                self.send_ops(ops)?;
            }
            Inbound::Welcome { rev, snapshot, ops, seen, history, peers, .. } => {
                // After a reconnect: catch up, then send what the relay didn't get.
                let doc = match snapshot {
                    Some(json) => Some(H::decode(&json).ok_or_else(|| format!("the shared document isn't a {} document", self.app.name))?),
                    None => None,
                };
                self.peers = peers.into_iter().map(|p| (p.client, (p.user, p.color))).collect();
                let ops = self.replica.resume(rev, doc.map(|d| (d, history)), ops, seen, session).map_err(|e| e.to_string())?;
                self.send_ops(ops)?;
            }
            Inbound::Join(p) | Inbound::Presence(p) => {
                self.peers.insert(p.client, (p.user, p.color));
            }
            Inbound::Leave { client } => {
                self.peers.remove(&client);
            }
            Inbound::Refused { seq, reason } => {
                self.replica.refused(seq, session);
                self.refused = Some(reason);
            }
            // An agent never holds the lease: nothing to save, upload or export.
            _ => {}
        }
        Ok(())
    }

    /// Take in what the relay sent, waiting up to `wait` for the first message.
    fn pump(&mut self, wait: Duration, session: &mut H::Session) -> Result<(), String> {
        let mut wait = wait;
        loop {
            match self.wire.recv(wait) {
                Ok(Some(msg)) => self.handle(&msg, session)?,
                Ok(None) => return Ok(()),
                Err(WireError::Lost(e)) => return self.reconnect_after(e, session),
                Err(WireError::Ended(e)) => {
                    self.ended = Some(e.clone());
                    return Err(e);
                }
            }
            wait = Duration::from_millis(1);
        }
    }

    /// The connection failed: open a new one and catch up (the `Welcome` resumes the replica).
    fn reconnect_after(&mut self, why: String, session: &mut H::Session) -> Result<(), String> {
        eprintln!("collab-mcp: {why}; reconnecting");
        self.wire = (self.reconnect)().map_err(|e| format!("{why}; reconnecting failed: {e}"))?;
        let hello = hello(self.app.id, &self.token, &self.client, Some(self.replica.rev()));
        self.wire.send(&hello)?;
        let deadline = Instant::now() + JOIN_WAIT;
        while Instant::now() < deadline {
            if let Some(msg) = self.wire.recv(Duration::from_millis(200)).map_err(WireError::message)? {
                let welcome = msg.contains(r#""t":"Welcome""#);
                self.handle(&msg, session)?;
                if welcome {
                    return Ok(());
                }
            }
        }
        Err(format!("{why}; the relay didn't welcome the agent back"))
    }

    /// Before a call: why the relay won't have the agent (every call fails with it), else take in
    /// what it sent.
    pub fn before(&mut self, session: &mut H::Session) -> Result<(), String> {
        if let Some(why) = &self.ended {
            return Err(why.clone());
        }
        self.pump(Duration::from_millis(1), session)
    }

    /// After a call: send the agent's edits and wait until the relay confirmed them, then say
    /// where its selection is. An edit the relay refused fails the call with its reason.
    pub fn after(&mut self, session: &mut H::Session) -> Result<(), String> {
        self.replica.drain_local(session);
        let deadline = Instant::now() + CONFIRM_WAIT;
        loop {
            if let Some(op) = self.replica.next_to_send() {
                self.send(&Outbound::Op(Box::new(op)))?;
            }
            if let Some(h) = self.replica.hash_due() {
                self.send(&h)?;
            }
            if self.replica.pending_len() == 0 {
                break;
            }
            if Instant::now() >= deadline {
                return Err("the relay didn't confirm the edit in 10 s; it will be sent again".into());
            }
            self.pump(Duration::from_millis(50), session)?;
        }
        if let Some(sel) = H::selection(session) {
            self.send(&Outbound::Presence { sel, rev: self.replica.rev() })?;
        }
        match self.refused.take() {
            Some(reason) => Err(reason),
            None => Ok(()),
        }
    }

    /// The agent's own methods (`collab.who`, `collab.wait`) and the ones it refuses while
    /// joined (opening or starting another document); `None` for the app's own methods.
    pub fn answer(&mut self, method: &str, params: &Value, session: &mut H::Session) -> Option<Result<Value, String>> {
        match method {
            "collab.who" => Some(Ok(self.who())),
            "collab.wait" => {
                let since = params.get("since").and_then(Value::as_u64).unwrap_or_else(|| self.replica.rev());
                let ms = params.get("timeoutMs").and_then(Value::as_u64).unwrap_or(30_000);
                Some(self.wait_for_changes(since, Duration::from_millis(ms), session))
            }
            // A shared document stays the one document: the agent can't open or start another.
            "file.new" | "file.open" | "file.close" | "file.newFromTemplate" => Some(Err(format!("{method}: not while joined to a shared document"))),
            _ => None,
        }
    }

    fn who(&self) -> Value {
        let people: Vec<Value> = self.peers.values().map(|(name, color)| json!({"name": name, "color": color})).collect();
        json!({"you": self.name, "people": people, "rev": self.replica.rev()})
    }

    /// Wait until the document is past revision `since` (or `timeout` passes) and list the
    /// changes after it.
    fn wait_for_changes(&mut self, since: u64, timeout: Duration, session: &mut H::Session) -> Result<Value, String> {
        let deadline = Instant::now() + timeout.min(MAX_WAIT);
        while self.replica.rev() <= since && Instant::now() < deadline {
            self.pump(Duration::from_millis(100), session)?;
        }
        let changes: Vec<Value> =
            self.changes.iter().filter(|c| c.rev > since).map(|c| json!({"rev": c.rev, "by": c.user, "command": c.command})).collect();
        Ok(json!({"rev": self.replica.rev(), "changes": changes}))
    }

    /// The relay this agent joined, for the MCP server's description.
    pub fn describe(&self) -> String {
        format!("joined the shared {} document at {} as {}", self.app.name, self.relay, self.name)
    }

    /// The shared revision the agent has.
    pub fn rev(&self) -> u64 {
        self.replica.rev()
    }
}

/// One MCP call through an agent: `session` reaches the backend's session, `run` is the
/// backend's own call. The agent's methods are answered here; the others run on the shared
/// document and sync before the reply.
pub fn call_on<H: LinkHost, B>(
    agent: &mut Agent<H>,
    backend: &mut B,
    session: impl Fn(&mut B) -> &mut H::Session,
    method: &str,
    params: Value,
    run: impl FnOnce(&mut B, &str, Value) -> Result<Value, String>,
) -> Result<Value, String> {
    agent.before(session(backend))?;
    let result = match agent.answer(method, &params, session(backend)) {
        Some(Ok(v)) if method == "collab.wait" => return Ok(v),
        Some(r) => r,
        None => run(backend, method, params),
    };
    agent.after(session(backend))?;
    result
}

pub mod fake;

#[cfg(test)]
mod tests;
