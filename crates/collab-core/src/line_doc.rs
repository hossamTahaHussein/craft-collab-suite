//! A tiny co-editable document for the core's own tests: a list of lines, a selection that is a
//! line index, and three commands (`line.insert` before the selected line, `line.delete`,
//! `line.set`). Positions move as line numbers do when lines come and go.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::doc::{CollabCtx, CollabDoc, Local};
use crate::proto::Op;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ctx {
    pub author: String,
    pub time: u64,
    #[serde(default)]
    pub api: bool,
}

impl CollabCtx for Ctx {
    fn author(&self) -> &str {
        &self.author
    }
    fn time(&self) -> u64 {
        self.time
    }
    fn track(&self) -> bool {
        false
    }
    fn api(&self) -> bool {
        self.api
    }
}

/// What a revision did to the lines: `old` lines from `at` became `new` lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Splice {
    pub at: usize,
    pub old: usize,
    pub new: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Wire {
    pub splice: Option<Splice>,
}

pub struct Session {
    pub lines: Vec<String>,
    pub sel: usize,
    pub recording: bool,
    pub author: String,
    pub time: u64,
    pub outbox: Vec<Local<LineDoc>>,
}

impl Session {
    pub fn new(lines: Vec<String>) -> Session {
        Session { lines, sel: 0, recording: true, author: String::new(), time: 0, outbox: Vec::new() }
    }

    /// Run a command as the person would: recorded for the replica when it changed something.
    pub fn run(&mut self, cmd: &str, params: Value) -> bool {
        let sel = self.sel;
        let ok = apply(&mut self.lines, &mut self.sel, cmd, &params);
        if ok && self.recording {
            self.outbox.push(Local { cmd: cmd.into(), params, sel, ctx: Ctx { author: self.author.clone(), time: self.time, api: false } });
        }
        ok
    }
}

fn apply(lines: &mut Vec<String>, sel: &mut usize, cmd: &str, params: &Value) -> bool {
    let text = params.get("text").and_then(Value::as_str).unwrap_or("").to_string();
    match cmd {
        "line.insert" if *sel <= lines.len() => {
            lines.insert(*sel, text);
            *sel += 1;
            true
        }
        "line.delete" if *sel < lines.len() => {
            lines.remove(*sel);
            true
        }
        "line.set" if *sel < lines.len() && lines[*sel] != text => {
            lines[*sel] = text;
            true
        }
        _ => false,
    }
}

fn splice(before: &[String], after: &[String]) -> Option<Splice> {
    if before == after {
        return None;
    }
    let p = before.iter().zip(after).take_while(|(a, b)| a == b).count();
    let s = before[p..].iter().rev().zip(after[p..].iter().rev()).take_while(|(a, b)| a == b).count();
    Some(Splice { at: p, old: before.len() - p - s, new: after.len() - p - s })
}

fn map_line(line: usize, s: &Splice) -> usize {
    if line <= s.at {
        line
    } else if line >= s.at + s.old {
        line + s.new - s.old
    } else {
        s.at + s.new
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineDoc;

impl CollabDoc for LineDoc {
    type Doc = Vec<String>;
    type Session = Session;
    type Sel = usize;
    type Ctx = Ctx;
    type Change = Option<Splice>;
    type Step = Splice;
    type Wire = Wire;
    type Hint = usize;

    fn headless(doc: Vec<String>) -> Session {
        let mut s = Session::new(doc);
        s.recording = false;
        s
    }
    fn doc(s: &Session) -> &Vec<String> {
        &s.lines
    }
    fn set_doc(s: &mut Session, doc: Vec<String>) {
        s.lines = doc;
    }
    fn attach(visible: &mut Session, doc: &Vec<String>) {
        visible.lines = doc.clone();
        visible.sel = 0;
    }
    fn take_local(visible: &mut Session) -> Vec<Local<LineDoc>> {
        std::mem::take(&mut visible.outbox)
    }
    fn selection(s: &Session) -> Option<usize> {
        Some(s.sel)
    }
    fn select(s: &mut Session, sel: &usize) {
        s.sel = *sel;
    }
    fn clamp(s: &mut Session) {
        s.sel = s.sel.min(s.lines.len());
    }
    fn settle(visible: &mut Session) {
        Self::clamp(visible);
    }
    fn begin_replay(visible: &mut Session) -> bool {
        std::mem::replace(&mut visible.recording, false)
    }
    fn end_replay(visible: &mut Session, saved: bool) {
        visible.recording = saved;
    }
    fn run_at(s: &mut Session, op: &Op<LineDoc>, sel: &usize) -> bool {
        s.sel = (*sel).min(s.lines.len());
        apply(&mut s.lines, &mut s.sel, &op.cmd, &op.params)
    }
    fn restore_into(s: &mut Session, op: &Op<LineDoc>) {
        if let Some(lines) = op.params.get("doc").and_then(|d| serde_json::from_value::<Vec<String>>(d.clone()).ok()) {
            s.lines = lines;
            Self::clamp(s);
        }
    }
    fn changes(before: &Vec<String>, after: &Vec<String>, _hint: Option<usize>) -> Option<Splice> {
        splice(before, after)
    }
    fn hint_at(sel: &usize) -> Option<usize> {
        Some(*sel)
    }
    fn hint_in(change: &Option<Splice>, _sel: &usize) -> Option<usize> {
        change.map(|s| s.at)
    }
    fn map(sel: &usize, change: &Option<Splice>) -> usize {
        change.as_ref().map_or(*sel, |s| map_line(*sel, s))
    }
    fn step(before: &Vec<String>, after: &Vec<String>, _sel: &usize, _hint: Option<usize>) -> Option<Splice> {
        splice(before, after)
    }
    fn move_sel(sel: &usize, step: Option<&Splice>) -> usize {
        step.map_or(*sel, |s| map_line(*sel, s))
    }
    fn move_hint(hint: Option<usize>, _sel: &usize, own: &Splice) -> Option<usize> {
        hint.map(|h| map_line(h, own))
    }
    fn after_insertion(sel: &usize, step: &Splice) -> Option<usize> {
        (step.old == 0 && *sel == step.at).then_some(step.at + step.new)
    }
    fn to_wire(change: &Option<Splice>) -> Wire {
        Wire { splice: *change }
    }
    fn from_wire(wire: Wire) -> Option<Splice> {
        wire.splice
    }
    fn hash(doc: &Vec<String>) -> String {
        doc.join("\n")
    }
    fn encode(doc: &Vec<String>) -> Result<String, String> {
        serde_json::to_string(doc).map_err(|e| e.to_string())
    }
    fn is_typing(op: &Op<LineDoc>) -> bool {
        op.cmd == "line.insert"
    }
    fn merge(_first: &mut Op<LineDoc>, _next: &Op<LineDoc>) -> bool {
        false
    }
}
