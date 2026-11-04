//! Other people's edits, applied in batches: each one applied lays the document out again, and
//! twenty people typing send a hundred a second. An edit or a selection that arrives after a
//! quiet spell goes through at once; while they keep coming, they go through together at most
//! every [`BATCH_MS`]. Every other message (a welcome, a resync, a save) goes through at once,
//! with everything held before it, so the order never changes.

use std::collections::VecDeque;

/// The longest an edit waits before it is applied.
pub const BATCH_MS: f64 = 100.0;

#[derive(Default)]
pub struct RemoteBatch {
    held: VecDeque<String>,
    /// When edits last went through.
    released: Option<f64>,
}

/// An edit or a selection: what may wait for the next batch.
fn batched(json: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct Kind<'a> {
        #[serde(borrow)]
        t: std::borrow::Cow<'a, str>,
    }
    serde_json::from_str::<Kind>(json).is_ok_and(|k| k.t == "Op" || k.t == "Presence")
}

impl RemoteBatch {
    pub fn push(&mut self, json: String) {
        self.held.push_back(json);
    }

    /// The messages to apply now (`now`: milliseconds), oldest first.
    pub fn release(&mut self, now: f64) -> Vec<String> {
        if self.held.is_empty() {
            return Vec::new();
        }
        let due = self.released.is_none_or(|at| now - at >= BATCH_MS);
        if due {
            self.released = Some(now);
        } else if self.held.iter().all(|m| batched(m)) {
            return Vec::new();
        }
        self.held.drain(..).collect()
    }

    /// How long until held edits go through, when some are held.
    pub fn wait(&self, now: f64) -> Option<f64> {
        let at = self.released?;
        (!self.held.is_empty()).then(|| (at + BATCH_MS - now).max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(rev: u64) -> String {
        format!(r#"{{"t":"Op","rev":{rev}}}"#)
    }

    #[test]
    fn an_edit_after_a_quiet_spell_goes_through_at_once() {
        let mut b = RemoteBatch::default();
        b.push(op(1));
        assert_eq!(b.release(0.0), [op(1)]);
        b.push(op(2));
        assert_eq!(b.release(500.0), [op(2)]);
    }

    #[test]
    fn edits_that_keep_coming_go_through_together() {
        let mut b = RemoteBatch::default();
        b.push(op(1));
        assert_eq!(b.release(0.0).len(), 1);
        b.push(op(2));
        assert!(b.release(16.0).is_empty(), "held");
        b.push(r#"{"t":"Presence","client":"a"}"#.into());
        b.push(op(3));
        assert!(b.release(50.0).is_empty(), "held");
        assert_eq!(b.wait(50.0), Some(50.0));
        assert_eq!(b.release(100.0), [op(2), r#"{"t":"Presence","client":"a"}"#.into(), op(3)], "in order");
        assert_eq!(b.wait(100.0), None);
    }

    #[test]
    fn other_messages_go_through_at_once_after_what_was_held() {
        let mut b = RemoteBatch::default();
        b.push(op(1));
        b.release(0.0);
        b.push(op(2));
        b.push(r#"{"t":"Save"}"#.into());
        assert_eq!(b.release(10.0), [op(2), r#"{"t":"Save"}"#.into()]);
    }
}
