use super::*;

#[test]
fn hello_says_it_is_an_agent_of_its_app_and_where_it_was() {
    let h: Value = serde_json::from_str(&hello("sheets", "tok", "agent1", None)).unwrap();
    assert_eq!(h, json!({"t": "hello", "token": "tok", "client": "agent1", "agent": true, "app": "sheets"}));
    let h: Value = serde_json::from_str(&hello("slides", "tok", "agent1", Some(7))).unwrap();
    assert_eq!(h["haveRev"], 7);
}

/// A bad client or token, the wrong app, removed or closed: the agent stays out, as the SDK does.
#[test]
fn which_closes_are_final() {
    for code in [4400, 4401, 4403, 4410] {
        assert!(matches!(WireError::closed(code, "why"), WireError::Ended(m) if m.contains("why")), "{code}");
    }
    for code in [1006, 1011, 4409] {
        assert!(matches!(WireError::closed(code, ""), WireError::Lost(_)), "{code}");
    }
}
