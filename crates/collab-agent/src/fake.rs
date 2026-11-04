//! A relay in memory, for testing an app's agent: what the agent sends lands in `sent`; `inbox`
//! is what it reads. `"CLOSE <code> <reason>"` in the inbox closes the connection.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::{Wire, WireError};

#[derive(Clone, Default)]
pub struct FakeRelay {
    pub sent: Rc<RefCell<Vec<Value>>>,
    pub inbox: Rc<RefCell<VecDeque<String>>>,
    /// Stamp the agent's ops and echo them back, as the relay does.
    pub echo: bool,
    /// Refuse the agent's ops with this reason instead.
    pub refuse: Option<String>,
    pub rev: Rc<RefCell<u64>>,
}

impl FakeRelay {
    /// A relay at revision `rev` whose first message is a `Welcome` with `snapshot` and one
    /// person (Ann) in it.
    pub fn welcoming(rev: u64, snapshot: &str) -> FakeRelay {
        let fake = FakeRelay { echo: true, ..FakeRelay::default() };
        *fake.rev.borrow_mut() = rev;
        let welcome = json!({"t": "Welcome", "rev": rev, "snapshot": snapshot, "ops": [], "you": "agent",
            "peers": [{"client": "p1", "user": "Ann", "color": 0}]});
        fake.inbox.borrow_mut().push_back(welcome.to_string());
        fake
    }

    /// The commands of the ops the agent sent.
    pub fn sent_commands(&self) -> Vec<String> {
        self.sent.borrow().iter().filter(|m| m["t"] == "Op").filter_map(|m| m["cmd"].as_str().map(str::to_string)).collect()
    }
}

impl Wire for FakeRelay {
    fn send(&mut self, msg: &str) -> Result<(), String> {
        let v: Value = serde_json::from_str(msg).map_err(|e| e.to_string())?;
        if let Some(reason) = self.refuse.as_ref().filter(|_| v["t"] == "Op") {
            self.inbox.borrow_mut().push_back(json!({"t": "Refused", "seq": v["seq"], "reason": reason}).to_string());
        } else if self.echo && v["t"] == "Op" {
            let mut rev = self.rev.borrow_mut();
            *rev += 1;
            let mut op = v.clone();
            op["rev"] = json!(*rev);
            op["user"] = json!(v["ctx"]["author"]);
            self.inbox.borrow_mut().push_back(op.to_string());
        }
        self.sent.borrow_mut().push(v);
        Ok(())
    }

    fn recv(&mut self, _wait: Duration) -> Result<Option<String>, WireError> {
        match self.inbox.borrow_mut().pop_front() {
            Some(m) if m.starts_with("CLOSE ") => {
                let (code, reason) = m[6..].split_once(' ').unwrap_or((&m[6..], ""));
                Err(WireError::closed(code.parse().unwrap_or(1006), reason))
            }
            m => Ok(m),
        }
    }
}
