//! Co-editing speed (suite spec row 45): someone else's cell edit shows in a 100,000-cell
//! workbook in under 4 ms. Timed in release builds (`cargo test --release -p gridcraft-collab
//! --test perf`); a debug build only checks it runs.

// Timed natively only: the wasm clock rule is for code that runs in the browser.
#![allow(clippy::disallowed_methods)]

mod support;

use std::sync::Arc;
use std::time::Instant;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Cell, Formula, Workbook};
use serde_json::json;
use support::{Peer, Relay};

/// 1,000 rows by 100 columns of numbers, and a column of totals reading each row.
fn big() -> Arc<Workbook> {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    for r in 0..1000u32 {
        for c in 0..100u32 {
            sh.set_cell(CellRef::new(r, c), Cell::value(Value::Number(f64::from(r * 100 + c))));
        }
        let mut total = Cell::value(Value::Number(0.0));
        total.formula = Some(Arc::new(Formula::new(&format!("SUM(A{0}:CV{0})", r + 1))));
        sh.set_cell(CellRef::new(r, 100), total);
    }
    Arc::new(wb)
}

#[test]
fn someone_elses_cell_edit_shows_in_a_big_workbook_quickly() {
    let mut relay = Relay::new();
    relay.base = big();
    let mut ann = Peer::join("a", "Ann", &relay);
    let mut bob = Peer::join("b", "Bob", &relay);
    let mut times = Vec::new();
    for k in 0..20 {
        ann.act("cell.set", json!({"cell": "C5", "input": (k + 1).to_string()}));
        relay.collect(&mut [&mut ann, &mut bob], &[0]);
        ann.deliver(usize::MAX);
        let t = Instant::now();
        bob.deliver(usize::MAX);
        times.push(t.elapsed());
    }
    assert_eq!(bob.value("C5"), Value::Number(20.0));
    assert_eq!(bob.value("CW5"), Value::Number((400..500).sum::<u32>() as f64 - 402.0 + 20.0), "the total follows");
    times.sort();
    let median = times[times.len() / 2];
    eprintln!("a remote cell edit in a 100,000-cell workbook: median {median:?}, worst {:?}", times.last().unwrap());
    if !cfg!(debug_assertions) {
        assert!(median.as_secs_f64() < 0.004, "median {median:?}");
    }
}

/// Twenty people typing into their own cells at once: every edit reaches everyone, each in well
/// under 4 ms, and all twenty workbooks end the same.
#[test]
fn twenty_people_typing_at_once_stay_quick_and_together() {
    let mut relay = Relay::new();
    relay.base = big();
    let mut people: Vec<Peer> = (0..20).map(|i| Peer::join(&format!("p{i}"), &format!("Person {i}"), &relay)).collect();
    let mut times = Vec::new();
    for round in 0..5 {
        for (i, p) in people.iter_mut().enumerate() {
            p.act("cell.set", json!({"cell": format!("D{}", i + 1), "input": format!("{round}")}));
        }
        let order: Vec<usize> = (0..people.len()).collect();
        let mut refs: Vec<&mut Peer> = people.iter_mut().collect();
        relay.collect(&mut refs, &order);
        for p in people.iter_mut() {
            while !p.inbox.is_empty() {
                let t = Instant::now();
                p.deliver(1);
                times.push(t.elapsed());
            }
        }
    }
    let h = people[0].visible_hash();
    assert!(people.iter().all(|p| p.visible_hash() == h), "everyone ends with the same workbook");
    times.sort();
    let median = times[times.len() / 2];
    eprintln!("20 people typing, a delivered edit: median {median:?}, worst {:?}", times.last().unwrap());
    if !cfg!(debug_assertions) {
        assert!(median.as_secs_f64() < 0.004, "median {median:?}");
    }
}
