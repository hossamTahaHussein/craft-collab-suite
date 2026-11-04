//! `deckcraft-cli collab-mcp --relay URL --token TOKEN [--name NAME]`: an agent in a shared
//! presentation, as Craft Docs' `wordcraft-cli collab-mcp` is. The MCP server's headless session
//! is a participant through `craft_collab_agent`: its edits sync live under its name and its
//! selection shows. (Suggesting isn't in Craft Slides yet: the agent's edits are direct, and a
//! suggester's token can't edit.)

use craft_collab_agent::{Agent, App, call_on};
use deckcraft_collab::SlidesDoc;
use deckcraft_engine::Session;
use deckcraft_mcp::{Backend, Headless};
use serde_json::Value;

/// Craft Slides for the agent.
pub fn app() -> App<SlidesDoc> {
    App { id: "slides", name: "Craft Slides", set_author: |s: &mut Session, name: &str| s.prefs.author = name.to_string() }
}

/// An MCP backend whose session is a participant in a shared presentation.
pub struct SlidesAgent {
    pub agent: Agent<SlidesDoc>,
    pub headless: Headless,
}

impl SlidesAgent {
    /// Join the relay at `relay` with join token `token`, as `name`.
    pub fn connect(relay: &str, token: &str, name: &str) -> Result<SlidesAgent, String> {
        let mut headless = Headless::new();
        let agent = Agent::connect(app(), relay, token, name, &mut headless.session)?;
        Ok(SlidesAgent { agent, headless })
    }
}

impl Backend for SlidesAgent {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        call_on(&mut self.agent, &mut self.headless, |h| &mut h.session, method, params, |h, m, p| h.call(m, p))
    }

    fn has_ui(&self) -> bool {
        false
    }

    fn describe(&self) -> String {
        self.agent.describe()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use craft_collab_agent::fake::FakeRelay;
    use craft_collab_core::doc::CollabDoc;
    use deckcraft_mcp::call_tool;
    use serde_json::json;

    use super::*;

    /// A presentation of `n` slides, as the relay keeps it.
    fn snapshot(n: usize) -> String {
        let mut s = Session::with_new();
        for _ in 1..n {
            s.execute("slide.new", &json!({})).unwrap();
        }
        SlidesDoc::encode(&s.active().unwrap().doc).unwrap()
    }

    fn joined(n: usize) -> (SlidesAgent, FakeRelay) {
        let fake = FakeRelay::welcoming(5, &snapshot(n));
        let mut headless = Headless::new();
        let agent = Agent::join(app(), Box::new(fake.clone()), Box::new(|| Err("no".into())), "tok", "Helper", &mut headless.session).unwrap();
        (SlidesAgent { agent, headless }, fake)
    }

    fn slides(a: &SlidesAgent) -> usize {
        a.headless.session.active().unwrap().doc.slides.len()
    }

    #[test]
    fn joins_the_shared_presentation_as_an_agent_and_says_who_is_here() {
        let (mut a, fake) = joined(2);
        let hello = fake.sent.borrow()[0].clone();
        assert_eq!((hello["agent"].as_bool(), hello["app"].as_str()), (Some(true), Some("slides")));
        assert_eq!(slides(&a), 2, "it has the shared presentation");
        let who = a.call("collab.who", json!({})).unwrap();
        assert_eq!((who["you"].as_str(), who["people"][0]["name"].as_str()), (Some("Helper"), Some("Ann")));
        assert!(!call_tool(&mut a, "who_is_here", &json!({})).is_error);
    }

    #[test]
    fn an_edit_syncs_under_the_agents_name_and_waits_for_its_echo() {
        let (mut a, fake) = joined(1);
        a.call("command", json!({"command": "slide.new"})).unwrap();
        assert_eq!(fake.sent_commands(), ["slide.new"]);
        let sent = fake.sent.borrow();
        let op = sent.iter().find(|m| m["t"] == "Op").unwrap();
        assert_eq!(op["ctx"]["author"], "Helper");
        assert!(sent.iter().any(|m| m["t"] == "Presence"), "people see where the agent is");
        drop(sent);
        assert_eq!(a.agent.rev(), 6, "confirmed before the reply");
        assert_eq!(slides(&a), 2);
    }

    #[test]
    fn others_edits_arrive_and_wait_for_changes_lists_them() {
        let (mut a, fake) = joined(1);
        // Ann's edit, as her editor sends it (another agent stands in for her).
        let (mut ann, ann_relay) = joined(1);
        ann.call("command", json!({"command": "slide.new"})).unwrap();
        let mut op = ann_relay.sent.borrow().iter().find(|m| m["t"] == "Op").cloned().unwrap();
        op["client"] = json!("p1");
        op["rev"] = json!(6);
        op["user"] = json!("Ann");
        fake.inbox.borrow_mut().push_back(op.to_string());
        let got = a.call("collab.wait", json!({"since": 5, "timeoutMs": 100})).unwrap();
        assert_eq!(got["changes"], json!([{"rev": 6, "by": "Ann", "command": "slide.new"}]));
        assert_eq!(slides(&a), 2);
    }

    #[test]
    fn a_refused_edit_is_the_calls_error_and_is_taken_back() {
        let mut fake = FakeRelay::welcoming(5, &snapshot(1));
        fake.refuse = Some("You can only comment on this document".into());
        let mut headless = Headless::new();
        let agent = Agent::join(app(), Box::new(fake), Box::new(|| Err("no".into())), "tok", "Helper", &mut headless.session).unwrap();
        let mut a = SlidesAgent { agent, headless };
        let err = a.call("command", json!({"command": "slide.new"})).unwrap_err();
        assert_eq!(err, "You can only comment on this document");
        assert_eq!(slides(&a), 1, "the refused edit is taken back");
    }

    #[test]
    fn a_closed_document_keeps_the_agent_out_and_another_one_is_refused() {
        let (mut a, fake) = joined(1);
        assert!(a.call("file.open", json!({"path": "x.pptx"})).unwrap_err().contains("shared document"));
        fake.inbox.borrow_mut().push_back("CLOSE 4410 This document was closed".into());
        for _ in 0..2 {
            assert!(a.call("collab.who", json!({})).unwrap_err().contains("closed"));
        }
    }
}
