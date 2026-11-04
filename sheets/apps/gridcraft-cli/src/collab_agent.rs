//! `gridcraft-cli collab-mcp --relay URL --token TOKEN [--name NAME] [--no-suggest]`: an agent in a shared
//! workbook, as Craft Docs' `wordcraft-cli collab-mcp` is. The MCP server's headless session is a
//! participant through `craft_collab_agent`: its edits sync live under its name and its cell
//! shows. Its edits arrive as suggestions people accept or reject, unless it was started with `--no-suggest`.

use craft_collab_agent::{Agent, App, call_on};
use gridcraft_collab::SheetsDoc;
use gridcraft_engine::Session;
use gridcraft_mcp::{Backend, Headless};
use serde_json::Value;

/// Craft Sheets for the agent.
pub fn app() -> App<SheetsDoc> {
    App { id: "sheets", name: "Craft Sheets", set_author: |s: &mut Session, name: &str| s.prefs.user_name = name.to_string() }
}

/// An MCP backend whose session is a participant in a shared workbook.
pub struct SheetsAgent {
    pub agent: Agent<SheetsDoc>,
    pub headless: Headless,
}

impl SheetsAgent {
    /// Join the relay at `relay` with join token `token`, as `name`.
    /// Its edits are suggestions while `suggesting`.
    pub fn connect(relay: &str, token: &str, name: &str, suggesting: bool) -> Result<SheetsAgent, String> {
        let mut headless = Headless::new();
        let agent = Agent::connect(app(), relay, token, name, &mut headless.session)?;
        headless.session.suggesting = suggesting;
        Ok(SheetsAgent { agent, headless })
    }
}

impl Backend for SheetsAgent {
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
    use gridcraft_engine::core::{CellRef, Value as Cell};
    use gridcraft_mcp::call_tool;
    use serde_json::json;

    use super::*;

    fn snapshot(a1: &str) -> String {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("cell.set", json!({"cell": "A1", "input": a1})).unwrap();
        SheetsDoc::encode(&s.active().unwrap().wb).unwrap()
    }

    fn joined(a1: &str) -> (SheetsAgent, FakeRelay) {
        let fake = FakeRelay::welcoming(5, &snapshot(a1));
        let mut headless = Headless::new();
        let agent = Agent::join(app(), Box::new(fake.clone()), Box::new(|| Err("no".into())), "tok", "Helper", &mut headless.session).unwrap();
        (SheetsAgent { agent, headless }, fake)
    }

    fn value(a: &SheetsAgent, at: &str) -> Cell {
        a.headless.session.active().unwrap().wb.active().unwrap().value(CellRef::parse(at).unwrap())
    }

    #[test]
    fn joins_the_shared_workbook_as_an_agent_and_says_who_is_here() {
        let (mut a, fake) = joined("Budget");
        let hello = fake.sent.borrow()[0].clone();
        assert_eq!((hello["agent"].as_bool(), hello["app"].as_str()), (Some(true), Some("sheets")));
        assert_eq!(value(&a, "A1"), Cell::Text("Budget".into()), "it has the shared workbook");
        let who = a.call("collab.who", json!({})).unwrap();
        assert_eq!((who["you"].as_str(), who["people"][0]["name"].as_str()), (Some("Helper"), Some("Ann")));
        assert!(!call_tool(&mut a, "who_is_here", &json!({})).is_error);
    }

    #[test]
    fn an_edit_syncs_under_the_agents_name_and_waits_for_its_echo() {
        let (mut a, fake) = joined("Budget");
        a.call("engine.execute", json!({"command": "cell.set", "params": {"cell": "B2", "input": "42"}})).unwrap();
        let sent = fake.sent.borrow();
        let op = sent.iter().find(|m| m["t"] == "Op").unwrap();
        assert_eq!((op["cmd"].as_str(), op["ctx"]["author"].as_str()), (Some("cell.set"), Some("Helper")));
        assert!(sent.iter().any(|m| m["t"] == "Presence"), "people see where the agent is");
        assert_eq!(a.agent.rev(), 6, "confirmed before the reply");
        drop(sent);
        assert_eq!(value(&a, "B2"), Cell::Number(42.0));
    }

    #[test]
    fn others_edits_arrive_and_wait_for_changes_lists_them() {
        let (mut a, fake) = joined("Budget");
        // Ann's edit, as her editor sends it (another agent stands in for her).
        let (mut ann, ann_relay) = joined("Budget");
        ann.call("engine.execute", json!({"command": "cell.set", "params": {"cell": "C3", "input": "7"}})).unwrap();
        let mut op = ann_relay.sent.borrow().iter().find(|m| m["t"] == "Op").cloned().unwrap();
        op["client"] = json!("p1");
        op["rev"] = json!(6);
        op["user"] = json!("Ann");
        fake.inbox.borrow_mut().push_back(op.to_string());
        let got = a.call("collab.wait", json!({"since": 5, "timeoutMs": 100})).unwrap();
        assert_eq!(got["changes"], json!([{"rev": 6, "by": "Ann", "command": "cell.set"}]));
        assert_eq!(value(&a, "C3"), Cell::Number(7.0));
    }

    #[test]
    fn a_refused_edit_is_the_calls_error_and_is_taken_back() {
        let mut fake = FakeRelay::welcoming(5, &snapshot("Budget"));
        fake.refuse = Some("You can only comment on this document".into());
        let mut headless = Headless::new();
        let agent = Agent::join(app(), Box::new(fake), Box::new(|| Err("no".into())), "tok", "Helper", &mut headless.session).unwrap();
        let mut a = SheetsAgent { agent, headless };
        let err = a.call("engine.execute", json!({"command": "cell.set", "params": {"cell": "B2", "input": "42"}})).unwrap_err();
        assert_eq!(err, "You can only comment on this document");
        assert_eq!(value(&a, "B2"), Cell::Empty, "the refused edit is taken back");
    }

    #[test]
    fn a_removed_agent_does_not_come_back_and_another_workbook_is_refused() {
        let (mut a, fake) = joined("Budget");
        assert!(a.call("file.open", json!({"path": "x.xlsx"})).unwrap_err().contains("shared document"));
        fake.inbox.borrow_mut().push_back("CLOSE 4403 Removed from this document".into());
        for _ in 0..2 {
            assert!(a.call("collab.who", json!({})).unwrap_err().contains("Removed"));
        }
    }

    #[test]
    fn nobody_there_yet_is_an_error() {
        let fake = FakeRelay::default();
        fake.inbox.borrow_mut().push_back(r#"{"t":"Welcome","rev":0,"snapshot":null,"ops":[],"you":"a"}"#.into());
        let mut h = Headless::new();
        let err = Agent::join(app(), Box::new(fake), Box::new(|| Err("no".into())), "t", "A", &mut h.session).err().unwrap();
        assert!(err.contains("nobody has opened"));
    }

    #[test]
    fn its_edits_are_suggestions_unless_told_otherwise() {
        let (mut a, fake) = joined("Budget");
        a.headless.session.suggesting = true;
        a.call("engine.execute", json!({"command": "cell.set", "params": {"cell": "B2", "input": "42"}})).unwrap();
        let op = fake.sent.borrow().iter().find(|m| m["t"] == "Op").cloned().unwrap();
        assert_eq!(op["ctx"]["track"], true, "a suggestion: {op}");
    }
}
