//! Tests for the dibs claim lease protocol, slice 1, translated
//! pin for pin from the Round 1 Python suite.
//!
//! Deterministic time comes only from injected instants. No test
//! sleeps to make a lease expire. The wall clock is read only by
//! the command line smoke path, where no expiry boundary is
//! crossed, and by the lock identity pin, which polls for the
//! worker's lock file and widens its window with a short sleep,
//! a wait on a file that judges no lease. Fixtures are synthetic
//! and written for this repo.
//!
//! Two reshapings from the port amendment land here. A value the
//! Python suite handed to an operation as the wrong type, an
//! integer or None for a name, a float or a string or a bool for
//! a time to live, a string for an epoch, is refused in the port
//! by the value level validator the operation itself calls first,
//! and those pins drive the validator directly for the shapes
//! the typed surface cannot express. The process race pins
//! reenter this test binary through an environment gate, the
//! shape the language offers for the same claims.

use std::path::{Path, PathBuf};
use std::process::Command;

use dibs::{
    check_possession, validate_name, validate_note, validate_ttl, Error, Instant, Item, OfferSpec,
    Registry, Value,
};

mod common;
use common::{at, expect_error, t0};

fn temp_dir(tag: &str) -> PathBuf {
    common::temp_dir("dibs-test-", tag)
}

fn make_registry(dir: &Path) -> Registry {
    Registry::new(dir.join("registry.json"))
}

fn assert_invalid_name<T>(result: dibs::Result<T>) {
    let error = expect_error(result);
    assert!(
        matches!(error, Error::InvalidName(_)),
        "expected InvalidName, got {error:?}"
    );
}

fn assert_invalid_ttl<T>(result: dibs::Result<T>) {
    let error = expect_error(result);
    assert!(
        matches!(error, Error::InvalidTtl(_)),
        "expected InvalidTtl, got {error:?}"
    );
}

fn assert_refusal<T>(result: dibs::Result<T>) {
    let error = expect_error(result);
    assert!(
        matches!(error, Error::Refusal(_)),
        "expected Refusal, got {error:?}"
    );
}

fn assert_not_found<T>(result: dibs::Result<T>) {
    let error = expect_error(result);
    assert!(
        matches!(error, Error::NotFound(_)),
        "expected NotFound, got {error:?}"
    );
}

fn assert_corrupt<T>(result: dibs::Result<T>) {
    let error = expect_error(result);
    assert!(
        matches!(error, Error::Corrupt(_)),
        "expected Corrupt, got {error:?}"
    );
}

#[test]
fn offer_claim_release_round_trip() {
    let dir = temp_dir("round-trip");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    let granted = registry.claim("task-one", "agent-a", 60, &at(1)).unwrap();
    assert_eq!(granted.epoch, 1);
    assert_eq!(granted.expires_at, at(61));
    assert_eq!(granted.token.len(), 43);
    let rows = registry.board(&at(2)).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "task-one");
    assert_eq!(rows[0].note, "a synthetic task");
    assert_eq!(rows[0].status, "claimed");
    assert_eq!(rows[0].holder.as_deref(), Some("agent-a"));
    assert_eq!(
        rows[0].expires_at.as_deref(),
        Some(dibs::format_instant(&at(61)).as_str())
    );
    assert_eq!(rows[0].epoch, 1);
    registry
        .release("task-one", "agent-a", &granted.token, 1, &at(3))
        .unwrap();
    let rows = registry.board(&at(4)).unwrap();
    assert_eq!(rows[0].status, "open");
    assert_eq!(rows[0].holder, None);
    assert_eq!(rows[0].expires_at, None);
    assert_eq!(rows[0].epoch, 1);
}

#[test]
fn full_life_history_is_gapless_and_typed() {
    let dir = temp_dir("full-life");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    let granted = registry.claim("task-one", "agent-a", 60, &at(1)).unwrap();
    registry
        .renew("task-one", "agent-a", &granted.token, 1, 60, &at(30))
        .unwrap();
    registry
        .release("task-one", "agent-a", &granted.token, 1, &at(40))
        .unwrap();
    let second = registry.claim("task-one", "agent-b", 60, &at(50)).unwrap();
    registry
        .complete("task-one", "agent-b", &second.token, 2, &at(55))
        .unwrap();
    let events = registry.history(None).unwrap();
    let seqs: Vec<i128> = events.iter().map(|event| event.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6]);
    let kinds: Vec<&str> = events.iter().map(|event| event.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "offered",
            "claimed",
            "renewed",
            "released",
            "claimed",
            "completed"
        ]
    );
    assert_eq!(events[0].holder, None);
    assert_eq!(events[0].epoch, 0);
    let epochs: Vec<i128> = events[1..].iter().map(|event| event.epoch).collect();
    assert_eq!(epochs, vec![1, 1, 1, 2, 2]);
    assert_eq!(registry.history(Some("task-one")).unwrap(), events);
    assert_eq!(registry.board(&at(56)).unwrap().len(), 0);
}

#[test]
fn claim_on_a_live_lease_refuses_and_names_the_holder() {
    let dir = temp_dir("live-refusal");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    registry.claim("task-one", "agent-a", 60, &at(1)).unwrap();
    let error = expect_error(registry.claim("task-one", "agent-b", 60, &at(2)));
    assert!(matches!(error, Error::Refusal(_)), "{error:?}");
    assert!(error.message().contains("agent-a"), "{}", error.message());
}

#[test]
fn expiry_boundary_is_the_expiry_instant_itself() {
    let dir = temp_dir("expiry-boundary");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    registry.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    let live = registry.board(&at(59)).unwrap();
    assert_eq!(live[0].status, "claimed");
    let boundary = registry.board(&at(60)).unwrap();
    assert_eq!(boundary[0].status, "open");
    assert_eq!(boundary[0].holder, None);
    assert_eq!(boundary[0].expires_at, None);
    assert_eq!(boundary[0].epoch, 1);
}

#[test]
fn renew_at_the_expiry_instant_is_refused() {
    let dir = temp_dir("renew-boundary");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    let granted = registry.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    let renewed = registry
        .renew("task-one", "agent-a", &granted.token, 1, 30, &at(59))
        .unwrap();
    assert_eq!(renewed, at(90));
    let second_registry = Registry::new(dir.join("second.json"));
    second_registry
        .offer("task-two", "a synthetic task", &t0())
        .unwrap();
    let granted_two = second_registry
        .claim("task-two", "agent-a", 60, &at(0))
        .unwrap();
    assert_refusal(second_registry.renew(
        "task-two",
        "agent-a",
        &granted_two.token,
        1,
        30,
        &at(60),
    ));
}

#[test]
fn renew_extends_from_the_current_expiry_not_from_now() {
    let dir = temp_dir("renew-extends");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    let granted = registry.claim("task-one", "agent-a", 100, &at(0)).unwrap();
    let expires = registry
        .renew("task-one", "agent-a", &granted.token, 1, 25, &at(90))
        .unwrap();
    assert_eq!(expires, at(125));
}

#[test]
fn lease_operations_touch_only_the_named_item() {
    let dir = temp_dir("named-item-only");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    registry
        .offer("task-two", "a synthetic task", &t0())
        .unwrap();
    let first = registry.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    let second = registry.claim("task-two", "agent-b", 60, &at(0)).unwrap();
    // Renewing the second item extends its lease from its own
    // standing expiry and leaves the first item's standing
    // lease exactly as it stood.
    let renewed = registry
        .renew("task-two", "agent-b", &second.token, 1, 30, &at(10))
        .unwrap();
    assert_eq!(renewed, at(90));
    let rows = registry.board(&at(11)).unwrap();
    assert_eq!(
        rows[0].expires_at.as_deref(),
        Some(dibs::format_instant(&at(60)).as_str())
    );
    assert_eq!(
        rows[1].expires_at.as_deref(),
        Some(dibs::format_instant(&at(90)).as_str())
    );
    // Releasing the first item opens it and leaves the second
    // item claimed under its renewed lease.
    registry
        .release("task-one", "agent-a", &first.token, 1, &at(12))
        .unwrap();
    let rows = registry.board(&at(13)).unwrap();
    assert_eq!(rows[0].status, "open");
    assert_eq!(rows[1].status, "claimed");
    assert_eq!(
        rows[1].expires_at.as_deref(),
        Some(dibs::format_instant(&at(90)).as_str())
    );
    // Completing the second item takes it off the board and
    // leaves the first item open at its standing epoch.
    registry
        .complete("task-two", "agent-b", &second.token, 1, &at(14))
        .unwrap();
    let rows = registry.board(&at(15)).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "task-one");
    assert_eq!(rows[0].status, "open");
    assert_eq!(rows[0].epoch, 1);
}

#[test]
fn takeover_records_the_previous_holder_and_fences_the_old_lease() {
    let dir = temp_dir("takeover");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    let first = registry.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    let second = registry.claim("task-one", "agent-b", 60, &at(60)).unwrap();
    assert_eq!(second.epoch, 2);
    assert_ne!(second.token, first.token);
    let events = registry.history(Some("task-one")).unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.kind, "taken_over");
    assert_eq!(last.holder.as_deref(), Some("agent-b"));
    assert_eq!(last.previous_holder.as_deref(), Some("agent-a"));
    assert_refusal(registry.renew("task-one", "agent-a", &first.token, 1, 30, &at(61)));
    assert_refusal(registry.release("task-one", "agent-a", &first.token, 1, &at(61)));
    assert_refusal(registry.complete("task-one", "agent-a", &first.token, 1, &at(61)));
    assert_refusal(registry.renew("task-one", "agent-a", &second.token, 1, 30, &at(61)));
    assert_refusal(registry.release("task-one", "agent-b", &second.token, 1, &at(61)));
    registry
        .release("task-one", "agent-b", &second.token, 2, &at(62))
        .unwrap();
}

#[test]
fn wrong_token_fails_even_with_the_right_holder_name() {
    let dir = temp_dir("wrong-token");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    registry.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    assert_refusal(registry.release("task-one", "agent-a", "not-the-token", 1, &at(1)));
    assert_refusal(registry.complete("task-one", "agent-a", "not-the-token", 1, &at(1)));
    assert_refusal(registry.renew("task-one", "agent-a", "not-the-token", 1, 30, &at(1)));
}

#[test]
fn wrong_holder_and_stale_epoch_fail() {
    let dir = temp_dir("wrong-holder-epoch");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    let granted = registry.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    assert_refusal(registry.release("task-one", "agent-b", &granted.token, 1, &at(1)));
    assert_refusal(registry.release("task-one", "agent-a", &granted.token, 7, &at(1)));
    // The Python pin also presented the epoch as the string "1",
    // a shape the typed surface cannot express, and the refusal
    // lives in the possession check the operations call, driven
    // here on the standing lease the claim recorded.
    let standing = Item {
        id: "task-one".to_string(),
        note: "a synthetic task".to_string(),
        status: "claimed".to_string(),
        holder: Some("agent-a".to_string()),
        token_hash: Some(dibs::token_hash(&granted.token)),
        epoch: 1,
        expires_at: Some(dibs::format_instant(&at(60))),
    };
    let error = expect_error(check_possession(
        &standing,
        "agent-a",
        &granted.token,
        &Value::Str("1".to_string()),
    ));
    assert!(matches!(error, Error::Refusal(_)), "{error:?}");
}

#[test]
fn epoch_mismatch_message_names_both_epochs_and_never_says_stale() {
    let dir = temp_dir("epoch-message");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    let granted = registry.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    let error = expect_error(registry.release("task-one", "agent-a", &granted.token, 7, &at(1)));
    let message = error.message();
    assert!(matches!(error, Error::Refusal(_)), "{error:?}");
    assert!(
        message.contains("does not match the standing epoch 1"),
        "{message}"
    );
    assert!(message.contains('7'), "{message}");
    assert!(!message.contains("stale"), "{message}");
    let standing = Item {
        id: "task-one".to_string(),
        note: "a synthetic task".to_string(),
        status: "claimed".to_string(),
        holder: Some("agent-a".to_string()),
        token_hash: Some(dibs::token_hash(&granted.token)),
        epoch: 1,
        expires_at: Some(dibs::format_instant(&at(60))),
    };
    let error = expect_error(check_possession(
        &standing,
        "agent-a",
        &granted.token,
        &Value::Str("1".to_string()),
    ));
    let message = error.message();
    assert!(
        message.contains("does not match the standing epoch 1"),
        "{message}"
    );
    assert!(!message.contains("stale"), "{message}");
}

#[test]
fn possession_holder_message_names_the_holderless_case() {
    // The possession check is public and the suite drives it
    // directly, so it can be handed an item that holds no
    // lease at all. Its holder refusal must name that case:
    // the item is held by no one, and the empty string the
    // message rendered in the holder's place before this pin
    // is a holder no name in the protocol can be.
    let open = Item {
        id: "task-one".to_string(),
        note: "a synthetic task".to_string(),
        status: "open".to_string(),
        holder: None,
        token_hash: None,
        epoch: 0,
        expires_at: None,
    };
    let error = expect_error(check_possession(&open, "agent-a", "tok", &Value::Int(0)));
    assert!(matches!(error, Error::Refusal(_)), "{error:?}");
    assert_eq!(
        error.message(),
        "item 'task-one' is held by no one, not by 'agent-a'"
    );
    // An item that carries a holder keeps the standing
    // sentence whole.
    let held = Item {
        id: "task-one".to_string(),
        note: "a synthetic task".to_string(),
        status: "claimed".to_string(),
        holder: Some("agent-a".to_string()),
        token_hash: Some(dibs::token_hash("tok")),
        epoch: 1,
        expires_at: Some(dibs::format_instant(&at(60))),
    };
    let error = expect_error(check_possession(&held, "agent-b", "tok", &Value::Int(1)));
    assert!(matches!(error, Error::Refusal(_)), "{error:?}");
    assert_eq!(
        error.message(),
        "item 'task-one' is held by 'agent-a', not by 'agent-b'"
    );
}

#[test]
fn the_registry_stores_only_the_token_hash() {
    let dir = temp_dir("hash-only");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    let granted = registry.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    let raw = std::fs::read(dir.join("registry.json")).unwrap();
    assert!(!raw
        .windows(granted.token.len())
        .any(|window| window == granted.token.as_bytes()));
    let document: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let item = &document["items"][0];
    assert_eq!(
        item["token_hash"].as_str().unwrap(),
        dibs::token_hash(&granted.token)
    );
    assert_ne!(item["token_hash"].as_str().unwrap(), granted.token);
}

#[test]
fn board_and_history_output_carry_no_token_or_hash() {
    let dir = temp_dir("no-token-output");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    let granted = registry.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    let rows = registry.board(&at(1)).unwrap();
    let board_text = serde_json::to_string(
        &rows
            .iter()
            .map(|row| {
                serde_json::json!({
                    "id": row.id, "note": row.note, "status": row.status,
                    "holder": row.holder, "expires_at": row.expires_at,
                    "epoch": row.epoch,
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let events = registry.history(None).unwrap();
    let history_text = serde_json::to_string(
        &events
            .iter()
            .map(|event| {
                serde_json::json!({
                    "seq": event.seq, "kind": event.kind, "item": event.item,
                    "holder": event.holder, "at": event.at, "epoch": event.epoch,
                    "previous_holder": event.previous_holder,
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let hash = dibs::token_hash(&granted.token);
    assert!(!board_text.contains(&granted.token));
    assert!(!history_text.contains(&granted.token));
    assert!(!board_text.contains(&hash));
    assert!(!history_text.contains(&hash));
}

#[test]
fn completion_is_terminal_and_the_id_cannot_be_reused() {
    let dir = temp_dir("completion-terminal");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    let granted = registry.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    registry
        .complete("task-one", "agent-a", &granted.token, 1, &at(1))
        .unwrap();
    assert_refusal(registry.claim("task-one", "agent-b", 60, &at(2)));
    assert_refusal(registry.renew("task-one", "agent-a", &granted.token, 1, 30, &at(2)));
    assert_refusal(registry.release("task-one", "agent-a", &granted.token, 1, &at(2)));
    assert_refusal(registry.complete("task-one", "agent-a", &granted.token, 1, &at(2)));
    assert_refusal(registry.offer("task-one", "a second life", &at(2)));
    let kinds: Vec<String> = registry
        .history(Some("task-one"))
        .unwrap()
        .iter()
        .map(|event| event.kind.clone())
        .collect();
    assert_eq!(kinds, vec!["offered", "claimed", "completed"]);
    assert_eq!(registry.board(&at(3)).unwrap().len(), 0);
}

#[test]
fn open_item_refuses_lease_operations_and_unknown_item_is_not_found() {
    let dir = temp_dir("open-and-ghost");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    assert_refusal(registry.renew("task-one", "agent-a", "tok", 0, 30, &at(1)));
    assert_refusal(registry.release("task-one", "agent-a", "tok", 0, &at(1)));
    assert_refusal(registry.complete("task-one", "agent-a", "tok", 0, &at(1)));
    assert_not_found(registry.claim("task-ghost", "agent-a", 60, &at(1)));
    assert_not_found(registry.renew("task-ghost", "agent-a", "tok", 1, 30, &at(1)));
    assert_not_found(registry.release("task-ghost", "agent-a", "tok", 1, &at(1)));
    assert_not_found(registry.complete("task-ghost", "agent-a", "tok", 1, &at(1)));
    assert_not_found(registry.history(Some("task-ghost")));
    assert_refusal(registry.offer("task-one", "a duplicate offer", &at(1)));
}

#[test]
fn a_second_claim_after_release_carries_a_fresh_token_and_epoch() {
    let dir = temp_dir("second-claim");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    let first = registry.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    registry
        .release("task-one", "agent-a", &first.token, 1, &at(1))
        .unwrap();
    let second = registry.claim("task-one", "agent-a", 60, &at(2)).unwrap();
    assert_eq!(second.epoch, 2);
    assert_ne!(second.token, first.token);
    assert_refusal(registry.release("task-one", "agent-a", &first.token, 2, &at(3)));
}

macro_rules! bad_item_name {
    ($name:ident, $value:expr) => {
        #[test]
        fn $name() {
            let dir = temp_dir(stringify!($name));
            let registry = make_registry(&dir);
            let bad: String = $value;
            assert_invalid_name(registry.offer(&bad, "a synthetic task", &t0()));
            assert_invalid_name(registry.claim(&bad, "agent-a", 60, &t0()));
            assert_invalid_name(registry.history(Some(&bad)));
        }
    };
}

bad_item_name!(invalid_item_name_empty, String::new());
bad_item_name!(invalid_item_name_upper, "Upper".to_string());
bad_item_name!(invalid_item_name_has_space, "has space".to_string());
bad_item_name!(invalid_item_name_leading_dash, "-leading".to_string());
bad_item_name!(invalid_item_name_trailing_dash, "trailing-".to_string());
bad_item_name!(invalid_item_name_leading_underscore, "_leading".to_string());
bad_item_name!(
    invalid_item_name_trailing_underscore,
    "trailing_".to_string()
);
bad_item_name!(invalid_item_name_slash, "a/b".to_string());
bad_item_name!(invalid_item_name_dot, "a.b".to_string());
bad_item_name!(invalid_item_name_too_long, "a".repeat(65));

#[test]
fn invalid_item_name_integer() {
    let error = expect_error(validate_name(&Value::Int(7), "item"));
    assert!(matches!(error, Error::InvalidName(_)), "{error:?}");
}

#[test]
fn invalid_item_name_none() {
    let error = expect_error(validate_name(&Value::Null, "item"));
    assert!(matches!(error, Error::InvalidName(_)), "{error:?}");
}

macro_rules! bad_holder_name {
    ($name:ident, $value:expr) => {
        #[test]
        fn $name() {
            let dir = temp_dir(stringify!($name));
            let registry = make_registry(&dir);
            let bad: String = $value;
            registry
                .offer("task-one", "a synthetic task", &t0())
                .unwrap();
            assert_invalid_name(registry.claim("task-one", &bad, 60, &t0()));
            assert_invalid_name(registry.renew("task-one", &bad, "tok", 1, 30, &t0()));
        }
    };
}

bad_holder_name!(invalid_holder_name_empty, String::new());
bad_holder_name!(invalid_holder_name_upper, "Upper".to_string());
bad_holder_name!(invalid_holder_name_has_space, "has space".to_string());
bad_holder_name!(invalid_holder_name_leading_dash, "-leading".to_string());
bad_holder_name!(invalid_holder_name_trailing_dash, "trailing-".to_string());
bad_holder_name!(
    invalid_holder_name_leading_underscore,
    "_leading".to_string()
);
bad_holder_name!(
    invalid_holder_name_trailing_underscore,
    "trailing_".to_string()
);
bad_holder_name!(invalid_holder_name_slash, "a/b".to_string());
bad_holder_name!(invalid_holder_name_dot, "a.b".to_string());
bad_holder_name!(invalid_holder_name_too_long, "a".repeat(65));

#[test]
fn invalid_holder_name_integer() {
    let error = expect_error(validate_name(&Value::Int(7), "holder"));
    assert!(matches!(error, Error::InvalidName(_)), "{error:?}");
}

#[test]
fn invalid_holder_name_none() {
    let error = expect_error(validate_name(&Value::Null, "holder"));
    assert!(matches!(error, Error::InvalidName(_)), "{error:?}");
}

#[test]
fn invalid_name_message_escapes_private_use_characters() {
    // Python counts the private use ranges as not printable in
    // every Unicode version, so its repr renders them as hex
    // escapes, and the refusal message does the same. On the code
    // before the fix the raw character stood in the message and
    // no escape appeared.
    let bmp = expect_error(validate_name(
        &Value::Str("agent-\u{e000}".to_string()),
        "holder",
    ));
    assert!(matches!(bmp, Error::InvalidName(_)), "{bmp:?}");
    assert!(bmp.message().contains("\\ue000"), "{}", bmp.message());
    assert!(!bmp.message().contains('\u{e000}'), "{}", bmp.message());
    let supplementary = expect_error(validate_name(
        &Value::Str("agent-\u{f0000}".to_string()),
        "holder",
    ));
    assert!(
        matches!(supplementary, Error::InvalidName(_)),
        "{supplementary:?}"
    );
    assert!(
        supplementary.message().contains("\\U000f0000"),
        "{}",
        supplementary.message()
    );
    assert!(
        !supplementary.message().contains('\u{f0000}'),
        "{}",
        supplementary.message()
    );
}

macro_rules! good_name {
    ($name:ident, $value:expr) => {
        #[test]
        fn $name() {
            let dir = temp_dir(stringify!($name));
            let registry = make_registry(&dir);
            let good: String = $value;
            registry.offer(&good, "a synthetic task", &t0()).unwrap();
            let granted = registry.claim(&good, &good, 60, &t0()).unwrap();
            assert_eq!(granted.epoch, 1);
        }
    };
}

good_name!(valid_name_single_letter, "a".to_string());
good_name!(valid_name_single_digit, "0".to_string());
good_name!(valid_name_mixed, "a-b_2".to_string());
good_name!(valid_name_agent, "agent-007".to_string());
good_name!(valid_name_at_the_bound, "a".repeat(64));

macro_rules! bad_ttl_typed {
    ($name:ident, $value:expr) => {
        #[test]
        fn $name() {
            let dir = temp_dir(stringify!($name));
            let registry = make_registry(&dir);
            registry
                .offer("task-one", "a synthetic task", &t0())
                .unwrap();
            assert_invalid_ttl(registry.claim("task-one", "agent-a", $value, &t0()));
            let granted = registry.claim("task-one", "agent-a", 60, &t0()).unwrap();
            assert_invalid_ttl(registry.renew(
                "task-one",
                "agent-a",
                &granted.token,
                1,
                $value,
                &at(1),
            ));
        }
    };
}

bad_ttl_typed!(invalid_ttl_zero, 0);
bad_ttl_typed!(invalid_ttl_negative, -5);

macro_rules! bad_ttl_value {
    ($name:ident, $value:expr) => {
        #[test]
        fn $name() {
            // A time to live of this shape reaches the validator
            // the claim and the renew both call first, and it
            // refuses there with the typed error, before any
            // state is read. The validator is a pure function
            // of the value, so the assertion stands alone:
            // staging a registry or claiming first could not
            // change its outcome.
            assert_invalid_ttl(validate_ttl(&$value));
        }
    };
}

bad_ttl_value!(invalid_ttl_float, Value::Float(1.5));
bad_ttl_value!(invalid_ttl_string, Value::Str("60".to_string()));
bad_ttl_value!(invalid_ttl_bool, Value::Bool(true));
bad_ttl_value!(invalid_ttl_none, Value::Null);

#[test]
fn ttl_ceiling_is_the_contract_number_and_is_enforced() {
    assert_eq!(dibs::TTL_MAX, 31536000);
    let dir = temp_dir("ttl-ceiling");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    let granted = registry
        .claim("task-one", "agent-a", dibs::TTL_MAX, &t0())
        .unwrap();
    assert_eq!(granted.expires_at, t0().plus_seconds(dibs::TTL_MAX));
    assert_invalid_ttl(registry.claim("task-one", "agent-b", dibs::TTL_MAX + 1, &at(1)));
    assert_invalid_ttl(registry.renew(
        "task-one",
        "agent-a",
        &granted.token,
        1,
        dibs::TTL_MAX + 1,
        &at(1),
    ));
}

#[test]
fn float_values_render_in_refusal_messages_as_python_renders_them() {
    // The messages that interpolate a value render a float the
    // way Python's repr renders it, and that rendering is an
    // equivalence obligation of the port. The cases stand on
    // the renderer's own decision boundaries: fixed notation
    // inside the exponent range and scientific outside it, a
    // trailing point zero on an integral fixed value, and the
    // named arms for negative zero and nan and the infinities.
    // A renderer that moved a boundary or dropped the point
    // zero would change one of these texts while every class
    // only pin around it still passed.
    let cases: [(f64, &str); 12] = [
        (1.5, "1.5"),
        (123.456, "123.456"),
        (2.0, "2.0"),
        (0.0001, "0.0001"),
        (0.00001, "1e-05"),
        (1e15, "1000000000000000.0"),
        (1e16, "1e+16"),
        (1e100, "1e+100"),
        (-0.0, "-0.0"),
        (f64::INFINITY, "inf"),
        (f64::NEG_INFINITY, "-inf"),
        (f64::NAN, "nan"),
    ];
    for (value, rendered) in cases {
        let error = expect_error(validate_ttl(&Value::Float(value)));
        assert!(matches!(error, Error::InvalidTtl(_)), "{error:?}");
        let expected = format!(
            "invalid time to live {rendered}, use a positive integer \
             number of seconds, at most 31536000"
        );
        assert_eq!(error.message(), expected.as_str(), "the float {value}");
    }
    // The name refusal interpolates the value the same way.
    let error = expect_error(validate_name(&Value::Float(1.5), "item"));
    assert!(matches!(error, Error::InvalidName(_)), "{error:?}");
    assert_eq!(
        error.message(),
        "invalid item id 1.5, use one component of lowercase letters \
         and digits and hyphens and underscores, starting and ending \
         with a letter or a digit, at most 64 characters long"
    );
}

macro_rules! huge_ttl {
    ($name:ident, $value:expr) => {
        #[test]
        fn $name() {
            let dir = temp_dir(stringify!($name));
            let registry = make_registry(&dir);
            registry
                .offer("task-one", "a synthetic task", &t0())
                .unwrap();
            assert_invalid_ttl(registry.claim("task-one", "agent-a", $value, &t0()));
            let granted = registry.claim("task-one", "agent-a", 60, &t0()).unwrap();
            assert_invalid_ttl(registry.renew(
                "task-one",
                "agent-a",
                &granted.token,
                1,
                $value,
                &at(1),
            ));
        }
    };
}

huge_ttl!(huge_ttl_trillion, 1_000_000_000_000i64);
huge_ttl!(huge_ttl_quintillion, 1_000_000_000_000_000_000i64);

#[test]
fn repeated_renewals_end_in_a_typed_error_at_the_representable_edge() {
    let dir = temp_dir("representable-edge");
    let path = dir.join("registry.json");
    let registry = Registry::new(path.clone());
    let start = Instant::from_civil(9999, 1, 1, 0, 0, 0, 0, Some(0)).unwrap();
    registry
        .offer("task-one", "a synthetic task", &start)
        .unwrap();
    let granted = registry
        .claim("task-one", "agent-a", 8_640_000, &start)
        .unwrap();
    let mut renewals = 0;
    loop {
        match registry.renew("task-one", "agent-a", &granted.token, 1, 8_640_000, &start) {
            Ok(_) => {
                renewals += 1;
                assert!(renewals < 10);
            }
            Err(error) => {
                assert!(matches!(error, Error::InvalidTtl(_)), "{error:?}");
                break;
            }
        }
    }
    assert_eq!(renewals, 2);
    let before = std::fs::read(&path).unwrap();
    assert_invalid_ttl(registry.renew("task-one", "agent-a", &granted.token, 1, 8_640_000, &start));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let kinds: Vec<String> = registry
        .history(Some("task-one"))
        .unwrap()
        .iter()
        .map(|event| event.kind.clone())
        .collect();
    assert_eq!(kinds, vec!["offered", "claimed", "renewed", "renewed"]);
}

#[test]
fn note_bound_is_enforced_at_the_bound() {
    let dir = temp_dir("note-bound");
    let registry = make_registry(&dir);
    registry.offer("task-one", &"n".repeat(200), &t0()).unwrap();
    let error = expect_error(registry.offer("task-two", &"n".repeat(201), &t0()));
    assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
    let error = expect_error(validate_note(&Value::Int(42)));
    assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
}

#[test]
fn a_naive_instant_is_refused() {
    let dir = temp_dir("naive-instant");
    let registry = make_registry(&dir);
    let naive = Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, None).unwrap();
    let error = expect_error(registry.offer("task-one", "a synthetic task", &naive));
    assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
    let error = expect_error(registry.board(&naive));
    assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
}

#[test]
fn an_os_failure_reading_the_registry_is_a_failure_not_corruption() {
    // The registry path stands as a directory, so reading it
    // fails in the operating system rather than in the document.
    // The error names the failure for what it is, where the read
    // as first built reported the same case as corruption.
    let dir = temp_dir("unreadable-registry");
    let path = dir.join("registry.json");
    std::fs::create_dir_all(&path).unwrap();
    let registry = Registry::new(path);
    let error = expect_error(registry.board(&t0()));
    assert!(matches!(error, Error::Failure(_)), "{error:?}");
    assert!(
        error.message().contains("the registry file cannot be read"),
        "{}",
        error.message()
    );
    assert_eq!(error.exit_code(), 1);
}

#[test]
fn missing_registry_reads_empty_and_the_first_mutation_creates_it() {
    let dir = temp_dir("missing-registry");
    let path = dir.join("deep").join("nested").join("registry.json");
    let registry = Registry::new(path.clone());
    assert_eq!(registry.board(&t0()).unwrap().len(), 0);
    assert_eq!(registry.history(None).unwrap().len(), 0);
    assert!(!path.exists());
    assert!(!dir.join("deep").exists());
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    assert!(path.is_file());
    let rows = registry.board(&t0()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "task-one");
}

#[test]
fn board_lists_items_in_offer_order() {
    let dir = temp_dir("board-order");
    let registry = make_registry(&dir);
    for name in ["task-c", "task-a", "task-b"] {
        registry.offer(name, "a synthetic task", &t0()).unwrap();
    }
    registry.claim("task-a", "agent-a", 60, &at(1)).unwrap();
    let rows = registry.board(&at(2)).unwrap();
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(ids, vec!["task-c", "task-a", "task-b"]);
    let statuses: Vec<&str> = rows.iter().map(|row| row.status.as_str()).collect();
    assert_eq!(statuses, vec!["open", "claimed", "open"]);
}

/// One staged edit to a valid registry document, used by the
/// damage family to break one field at a time.
type DamageEdit = Box<dyn Fn(&mut serde_json::Value)>;

fn fresh_valid_dir(base: &Path, name: &str) -> PathBuf {
    let directory = base.join(name);
    std::fs::create_dir_all(&directory).unwrap();
    let registry = Registry::new(directory.join("registry.json"));
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    registry.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    directory
}

fn valid_text(directory: &Path) -> String {
    std::fs::read_to_string(directory.join("registry.json")).unwrap()
}

fn assert_corrupt_and_unchanged(directory: &Path, text: &str) {
    let path = directory.join("registry.json");
    std::fs::write(&path, text).unwrap();
    let before = std::fs::read(&path).unwrap();
    let registry = Registry::new(path.clone());
    assert_corrupt(registry.board(&at(1)));
    assert_corrupt(registry.history(None));
    assert_corrupt(registry.offer("task-two", "a synthetic task", &at(1)));
    assert_corrupt(registry.claim("task-one", "agent-b", 60, &at(1)));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

fn mutated(base: &Path, name: &str, change: impl Fn(&mut serde_json::Value)) -> (PathBuf, String) {
    let directory = fresh_valid_dir(base, name);
    let mut document: serde_json::Value = serde_json::from_str(&valid_text(&directory)).unwrap();
    change(&mut document);
    (directory, serde_json::to_string(&document).unwrap())
}

#[test]
fn damage_duplicate_keys_at_any_level() {
    let base = temp_dir("damage-duplicate");
    let root_dir = fresh_valid_dir(&base, "root-case");
    let text = valid_text(&root_dir);
    let raw_duplicate = text.replacen(
        "\"format\": \"dibs-registry/1\"",
        "\"format\": \"dibs-registry/1\", \"format\": \"dibs-registry/1\"",
        1,
    );
    assert_ne!(raw_duplicate, text);
    assert_corrupt_and_unchanged(&root_dir, &raw_duplicate);
    let nested_dir = fresh_valid_dir(&base, "nested-case");
    let nested = valid_text(&nested_dir).replacen(
        "\"note\": \"a synthetic task\"",
        "\"note\": \"a synthetic task\", \"note\": \"a synthetic task\"",
        1,
    );
    assert_ne!(nested, valid_text(&nested_dir));
    assert_corrupt_and_unchanged(&nested_dir, &nested);
}

#[test]
fn damage_wrong_format_name() {
    // DB-S3 mechanical update (DB-21): "dibs-registry/2" is now a supported
    // format version, so pinning the unsupported-format branch uses
    // "dibs-registry/99".
    let base = temp_dir("damage-format");
    let (directory, text) = mutated(&base, "case", |doc| {
        doc["format"] = serde_json::json!("dibs-registry/99");
    });
    assert_corrupt_and_unchanged(&directory, &text);
}

#[test]
fn damage_wrong_types() {
    let base = temp_dir("damage-types");
    let cases: Vec<DamageEdit> = vec![
        Box::new(|doc| doc["items"] = serde_json::json!({})),
        Box::new(|doc| doc["history"] = serde_json::json!("none")),
        Box::new(|doc| doc["items"][0]["epoch"] = serde_json::json!("1")),
        Box::new(|doc| doc["items"][0]["epoch"] = serde_json::json!(true)),
        Box::new(|doc| doc["items"][0]["holder"] = serde_json::json!(7)),
        Box::new(|doc| doc["items"][0]["expires_at"] = serde_json::json!(99)),
        Box::new(|doc| doc["items"][0]["expires_at"] = serde_json::json!("not an instant")),
        Box::new(|doc| doc["items"][0]["expires_at"] = serde_json::json!("2031-05-04T12:00:00")),
        Box::new(|doc| doc["items"][0]["status"] = serde_json::json!([])),
        Box::new(|doc| doc["history"][0]["kind"] = serde_json::json!({})),
        Box::new(|doc| doc["history"][0]["seq"] = serde_json::json!("1")),
    ];
    for (index, change) in cases.iter().enumerate() {
        let (directory, text) = mutated(&base, &format!("case-{index}"), change);
        assert_corrupt_and_unchanged(&directory, &text);
    }
}

#[test]
fn damage_unknown_fields() {
    let base = temp_dir("damage-unknown");
    let cases: Vec<DamageEdit> = vec![
        Box::new(|doc| doc["extra"] = serde_json::json!(1)),
        Box::new(|doc| doc["items"][0]["priority"] = serde_json::json!(1)),
        Box::new(|doc| doc["history"][0]["comment"] = serde_json::json!("hello")),
        Box::new(|doc| {
            doc["items"][0].as_object_mut().unwrap().remove("epoch");
        }),
        Box::new(|doc| {
            doc.as_object_mut().unwrap().remove("history");
        }),
    ];
    for (index, change) in cases.iter().enumerate() {
        let (directory, text) = mutated(&base, &format!("case-{index}"), change);
        assert_corrupt_and_unchanged(&directory, &text);
    }
}

#[test]
fn damage_inconsistent_lease_shapes() {
    let base = temp_dir("damage-leases");
    let cases: Vec<DamageEdit> = vec![
        Box::new(|doc| doc["items"][0]["status"] = serde_json::json!("open")),
        Box::new(|doc| doc["items"][0]["status"] = serde_json::json!("paused")),
        Box::new(|doc| doc["items"][0]["token_hash"] = serde_json::Value::Null),
        Box::new(|doc| doc["items"][0]["token_hash"] = serde_json::json!("not-a-hash")),
        Box::new(|doc| doc["items"][0]["epoch"] = serde_json::json!(0)),
        Box::new(|doc| doc["items"][0]["id"] = serde_json::json!("Bad Name")),
    ];
    for (index, change) in cases.iter().enumerate() {
        let (directory, text) = mutated(&base, &format!("case-{index}"), change);
        assert_corrupt_and_unchanged(&directory, &text);
    }
}

#[test]
fn damage_history_shapes() {
    let base = temp_dir("damage-history");
    let cases: Vec<DamageEdit> = vec![
        Box::new(|doc| doc["history"][1]["seq"] = serde_json::json!(9)),
        Box::new(|doc| doc["history"][1]["kind"] = serde_json::json!("stolen")),
        Box::new(|doc| doc["history"][1]["previous_holder"] = serde_json::json!("agent-z")),
        Box::new(|doc| doc["history"][0]["holder"] = serde_json::json!("agent-a")),
        Box::new(|doc| doc["history"][0]["epoch"] = serde_json::json!(3)),
        Box::new(|doc| {
            let copy = doc["items"][0].clone();
            doc["items"].as_array_mut().unwrap().push(copy);
        }),
    ];
    for (index, change) in cases.iter().enumerate() {
        let (directory, text) = mutated(&base, &format!("case-{index}"), change);
        assert_corrupt_and_unchanged(&directory, &text);
    }
}

#[test]
fn damage_truncated_and_empty_documents() {
    let base = temp_dir("damage-truncated");
    let directory = fresh_valid_dir(&base, "cut-case");
    let text = valid_text(&directory);
    assert_corrupt_and_unchanged(&directory, &text[..text.len() / 2]);
    let empty_dir = fresh_valid_dir(&base, "empty-case");
    assert_corrupt_and_unchanged(&empty_dir, "");
    let list_dir = fresh_valid_dir(&base, "list-case");
    assert_corrupt_and_unchanged(&list_dir, "[1, 2, 3]");
}

#[test]
fn damage_event_holder_name_message_names_the_event() {
    let base = temp_dir("damage-event-holder");
    let (directory, text) = mutated(&base, "case", |doc| {
        doc["history"][1]["holder"] = serde_json::json!("Bad Name");
    });
    let path = directory.join("registry.json");
    std::fs::write(&path, text).unwrap();
    let registry = Registry::new(path);
    let error = expect_error(registry.board(&at(1)));
    assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
    assert!(
        error
            .message()
            .contains("a history event holds an invalid holder id"),
        "{}",
        error.message()
    );
}

/// A registry document holding one claimed item under a known
/// token, with its expiry spelled in the canonical form, as raw
/// text the encoding pins below re-encode.
fn encodable_claimed_document() -> String {
    format!(
        "{{\"format\": \"dibs-registry/1\", \"items\": [{{\"id\": \"task-one\", \
         \"note\": \"abc\", \"status\": \"claimed\", \"holder\": \"agent-a\", \
         \"token_hash\": \"{}\", \"epoch\": 1, \"expires_at\": \"2031-05-04T12:01:00+00:00\"}}], \
         \"history\": []}}",
        dibs::token_hash("standing-token")
    )
}

/// Encode a document text under one byte order mark, the way a
/// writer of that encoding would have written it.
fn encode_document(text: &str, mark: &str) -> Vec<u8> {
    let mut out = Vec::new();
    match mark {
        "utf-8" => {
            out.extend_from_slice(&[0xef, 0xbb, 0xbf]);
            out.extend_from_slice(text.as_bytes());
        }
        "utf-16le" => {
            out.extend_from_slice(&[0xff, 0xfe]);
            for unit in text.encode_utf16() {
                out.extend_from_slice(&unit.to_le_bytes());
            }
        }
        "utf-16be" => {
            out.extend_from_slice(&[0xfe, 0xff]);
            for unit in text.encode_utf16() {
                out.extend_from_slice(&unit.to_be_bytes());
            }
        }
        "utf-32le" => {
            out.extend_from_slice(&[0xff, 0xfe, 0x00, 0x00]);
            for ch in text.chars() {
                out.extend_from_slice(&(ch as u32).to_le_bytes());
            }
        }
        _ => {
            out.extend_from_slice(&[0x00, 0x00, 0xfe, 0xff]);
            for ch in text.chars() {
                out.extend_from_slice(&(ch as u32).to_be_bytes());
            }
        }
    }
    out
}

#[test]
fn registry_documents_with_a_byte_order_mark_read_in_each_encoding() {
    // The reader decodes the registry bytes the way the Python
    // json module decoded them, detecting a byte order mark
    // and transcoding UTF-16 and UTF-32, so a registry written
    // in any of those encodings reads as the document it
    // encodes. The dispatch carries one ordering trap: the
    // UTF-32 little endian mark opens with the UTF-16 little
    // endian mark, so the four byte check must run first, and
    // a dispatch that reversed them would decode a UTF-32
    // document as UTF-16 text full of NUL characters and
    // refuse it as corruption, while every UTF-8 pin around
    // this one still passed. Each encoding below must read its
    // board at both sides of the lease's expiry boundary.
    let text = encodable_claimed_document();
    for mark in ["utf-8", "utf-16le", "utf-16be", "utf-32le", "utf-32be"] {
        let dir = temp_dir(&format!("bom-{mark}"));
        let path = dir.join("registry.json");
        std::fs::write(&path, encode_document(&text, mark)).unwrap();
        let registry = Registry::new(path);
        let live = registry.board(&at(30)).unwrap();
        assert_eq!(live.len(), 1, "{mark}");
        assert_eq!(live[0].status, "claimed", "{mark}");
        assert_eq!(
            live[0].expires_at.as_deref(),
            Some("2031-05-04T12:01:00+00:00"),
            "{mark}"
        );
        let expired = registry.board(&at(60)).unwrap();
        assert_eq!(expired[0].status, "open", "{mark}");
    }
}

#[test]
fn registry_documents_in_a_broken_encoding_refuse_as_corruption() {
    // The broken forms of the same encodings refuse as
    // corruption and leave the registry bytes standing: a
    // UTF-16 document with an odd byte count, one carrying a
    // lone surrogate, a UTF-32 document carrying a count past
    // the last codepoint, and a UTF-8 mark followed by bytes
    // that do not decode. Each defect is staged inside a
    // whole document, in the note's text, so a decoder that
    // guessed at it, dropping the incomplete unit or reading
    // past the bad one, would produce a valid document and
    // stand caught; the strict decoder refuses at the decode
    // itself. The document text is plain ASCII, so a
    // character's offset in the text is its unit's offset in
    // both encodings.
    let base = temp_dir("broken-encodings");
    let text = encodable_claimed_document();
    let note_b = text.find("\"abc\"").unwrap() + 2;
    let mut odd_utf16 = encode_document(&text, "utf-16le");
    odd_utf16.push(0);
    let mut surrogate_units: Vec<u16> = text.encode_utf16().collect();
    surrogate_units[note_b] = 0xd800;
    let mut lone_surrogate = vec![0xff, 0xfe];
    for unit in &surrogate_units {
        lone_surrogate.extend_from_slice(&unit.to_le_bytes());
    }
    let mut past_last_codepoint = vec![0xff, 0xfe, 0x00, 0x00];
    for (index, ch) in text.chars().enumerate() {
        let value = if index == note_b {
            0x0011_0000
        } else {
            ch as u32
        };
        past_last_codepoint.extend_from_slice(&value.to_le_bytes());
    }
    let mut utf8_bytes = text.as_bytes().to_vec();
    utf8_bytes[note_b] = 0xff;
    let mut bad_utf8 = vec![0xef, 0xbb, 0xbf];
    bad_utf8.extend_from_slice(&utf8_bytes);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("odd-utf16", odd_utf16),
        ("lone-surrogate", lone_surrogate),
        ("past-last-codepoint", past_last_codepoint),
        ("bad-utf8", bad_utf8),
    ];
    for (name, bytes) in cases {
        let directory = base.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("registry.json");
        std::fs::write(&path, &bytes).unwrap();
        let registry = Registry::new(path.clone());
        assert_corrupt(registry.board(&at(1)));
        assert_corrupt(registry.history(None));
        assert_corrupt(registry.offer("task-two", "a synthetic task", &at(1)));
        assert_corrupt(registry.claim("task-one", "agent-b", 60, &at(1)));
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{name}");
    }
}

/// A registry holding two full lives, written by the library
/// itself. One item stands claimed by its third holder after a
/// release and a takeover, and one item stands completed. The
/// cross validation damage family edits this document one
/// disagreement at a time.
fn rich_valid_dir(base: &Path, name: &str) -> PathBuf {
    let directory = base.join(name);
    std::fs::create_dir_all(&directory).unwrap();
    let registry = Registry::new(directory.join("registry.json"));
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    let first = registry.claim("task-one", "agent-a", 60, &at(1)).unwrap();
    registry
        .renew("task-one", "agent-a", &first.token, 1, 60, &at(2))
        .unwrap();
    registry
        .release("task-one", "agent-a", &first.token, 1, &at(3))
        .unwrap();
    registry.claim("task-one", "agent-b", 60, &at(4)).unwrap();
    // The second lease runs out at second 64, so the claim at
    // second 200 takes the item over.
    let third = registry.claim("task-one", "agent-c", 60, &at(200)).unwrap();
    registry
        .renew("task-one", "agent-c", &third.token, 3, 60, &at(201))
        .unwrap();
    registry
        .offer("task-two", "another synthetic task", &t0())
        .unwrap();
    let done = registry.claim("task-two", "agent-a", 60, &at(1)).unwrap();
    registry
        .complete("task-two", "agent-a", &done.token, 1, &at(2))
        .unwrap();
    directory
}

/// Renumber the history of an edited document so the sequence
/// still grows by 1 from 1, which the per field validation
/// demands before the cross validation ever runs.
fn renumber(doc: &mut serde_json::Value) {
    for (index, event) in doc["history"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        event["seq"] = serde_json::json!(index + 1);
    }
}

fn cross_mutated(
    base: &Path,
    name: &str,
    change: impl Fn(&mut serde_json::Value),
) -> (PathBuf, String) {
    let directory = rich_valid_dir(base, name);
    let mut document: serde_json::Value = serde_json::from_str(&valid_text(&directory)).unwrap();
    change(&mut document);
    renumber(&mut document);
    (directory, serde_json::to_string(&document).unwrap())
}

#[test]
fn cross_validation_damage_shapes() {
    let base = temp_dir("damage-cross");
    let cases: Vec<DamageEdit> = vec![
        // An event names an item that does not stand.
        Box::new(|doc| {
            doc["history"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "seq": 99, "kind": "offered", "item": "task-ghost", "holder": null,
                    "at": "2031-05-04T12:00:00+00:00", "epoch": 0, "previous_holder": null
                }));
        }),
        // A claim carries the standing epoch of its chain
        // instead of one past it.
        Box::new(|doc| doc["history"][4]["epoch"] = serde_json::json!(1)),
        // An item is offered a second time.
        Box::new(|doc| {
            let offer = doc["history"][0].clone();
            doc["history"].as_array_mut().unwrap().insert(1, offer);
        }),
        // An event stands after the item completed.
        Box::new(|doc| {
            doc["history"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "seq": 99, "kind": "renewed", "item": "task-two", "holder": "agent-a",
                    "at": "2031-05-04T12:00:00+00:00", "epoch": 1, "previous_holder": null
                }));
        }),
        // A plain claim stands where a lease already stands,
        // a shape the writers record as a takeover.
        Box::new(|doc| doc["history"][6]["kind"] = serde_json::json!("claimed")),
        // A takeover stands where no lease stands.
        Box::new(|doc| {
            doc["history"][4]["kind"] = serde_json::json!("taken_over");
            doc["history"][4]["previous_holder"] = serde_json::json!("agent-a");
        }),
        // A takeover names a previous holder who does not stand.
        Box::new(|doc| doc["history"][5]["previous_holder"] = serde_json::json!("agent-z")),
        // A renew stands where no lease stands.
        Box::new(|doc| doc["history"][4]["kind"] = serde_json::json!("renewed")),
        // A release stands where no lease stands.
        Box::new(|doc| doc["history"][4]["kind"] = serde_json::json!("released")),
        // A completion stands where no lease stands.
        Box::new(|doc| doc["history"][4]["kind"] = serde_json::json!("completed")),
        // A renew names a holder who does not stand.
        Box::new(|doc| doc["history"][2]["holder"] = serde_json::json!("agent-b")),
        // A release names a holder who does not stand.
        Box::new(|doc| doc["history"][3]["holder"] = serde_json::json!("agent-b")),
        // A claim jumps the epoch by more than one.
        Box::new(|doc| doc["history"][1]["epoch"] = serde_json::json!(3)),
        // A renew carries an epoch that does not stand.
        Box::new(|doc| doc["history"][2]["epoch"] = serde_json::json!(2)),
        // The standing item reads open where its history ends
        // claimed.
        Box::new(|doc| {
            doc["items"][0]["status"] = serde_json::json!("open");
            doc["items"][0]["holder"] = serde_json::Value::Null;
            doc["items"][0]["token_hash"] = serde_json::Value::Null;
            doc["items"][0]["expires_at"] = serde_json::Value::Null;
        }),
        // The standing item carries an epoch its history never
        // reached.
        Box::new(|doc| doc["items"][0]["epoch"] = serde_json::json!(4)),
        // The standing item names a holder its history never
        // ended with.
        Box::new(|doc| doc["items"][0]["holder"] = serde_json::json!("agent-z")),
        // A takeover opens the recorded history at epoch 1,
        // where no lease it could take over ever stood, since
        // a first claim carries epoch 1 and a takeover carries
        // one past the claim it replaces.
        Box::new(|doc| {
            let history = doc["history"].as_array_mut().unwrap();
            history.truncate(7);
            history.push(serde_json::json!({
                "seq": 99, "kind": "taken_over", "item": "task-two", "holder": "agent-a",
                "at": "2031-05-04T12:00:00+00:00", "epoch": 1, "previous_holder": "agent-z"
            }));
            doc["items"][1]["status"] = serde_json::json!("claimed");
            doc["items"][1]["holder"] = serde_json::json!("agent-a");
            doc["items"][1]["token_hash"] = serde_json::json!(dibs::token_hash("staged-token"));
            doc["items"][1]["expires_at"] = serde_json::json!("2031-05-04T12:01:00+00:00");
        }),
    ];
    for (index, change) in cases.iter().enumerate() {
        let (directory, text) = cross_mutated(&base, &format!("case-{index}"), change);
        assert_corrupt_and_unchanged(&directory, &text);
    }
}

fn cross_error(base: &Path, name: &str, change: impl Fn(&mut serde_json::Value)) -> Error {
    let (directory, text) = cross_mutated(base, name, change);
    let path = directory.join("registry.json");
    std::fs::write(&path, text).unwrap();
    let registry = Registry::new(path);
    expect_error(registry.board(&at(1)))
}

#[test]
fn cross_validation_orphan_event_names_the_item() {
    let base = temp_dir("cross-orphan");
    let error = cross_error(&base, "case", |doc| {
        doc["history"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "seq": 99, "kind": "offered", "item": "task-ghost", "holder": null,
                "at": "2031-05-04T12:00:00+00:00", "epoch": 0, "previous_holder": null
            }));
    });
    assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
    assert!(
        error
            .message()
            .contains("names item 'task-ghost', which does not stand in the backlog"),
        "{}",
        error.message()
    );
}

#[test]
fn cross_validation_claim_epoch_message_names_both_epochs() {
    let base = temp_dir("cross-claim-epoch");
    let error = cross_error(&base, "case", |doc| {
        doc["history"][1]["epoch"] = serde_json::json!(3);
    });
    assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
    assert!(
        error
            .message()
            .contains("carries epoch 3, not one past the standing epoch 0"),
        "{}",
        error.message()
    );
}

#[test]
fn cross_validation_final_holder_message_names_both_holders() {
    let base = temp_dir("cross-final-holder");
    let error = cross_error(&base, "case", |doc| {
        doc["items"][0]["holder"] = serde_json::json!("agent-z");
    });
    assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
    assert!(
        error
            .message()
            .contains("stands held by 'agent-z', not by 'agent-c' as its history ends"),
        "{}",
        error.message()
    );
}

#[test]
fn cross_validation_offer_message_names_the_case_that_fires_it() {
    // An offer that arrives where the chain already holds an
    // offer is a second offer, and the refusal says so. The
    // damage family stages that case by repeating the first
    // recorded event, and the sentence is the standing one.
    let base = temp_dir("cross-offer-twice");
    let error = cross_error(&base, "case", |doc| {
        let offer = doc["history"][0].clone();
        doc["history"].as_array_mut().unwrap().insert(1, offer);
    });
    assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
    assert!(
        error.message().contains("is offered twice in its history"),
        "{}",
        error.message()
    );

    // A history that opens in the middle of a life holds no
    // offer at all, so an offer that arrives after the opening
    // event is the first offer the record carries, standing
    // away from the first place. The refusal names that case
    // and does not call the item offered twice.
    let dir = temp_dir("cross-offer-misplaced");
    let document = serde_json::json!({
        "format": "dibs-registry/1",
        "items": [{
            "id": "task-one", "note": "", "status": "claimed", "holder": "agent-a",
            "token_hash": dibs::token_hash("mid-life-token"), "epoch": 1,
            "expires_at": "2031-05-04T12:01:00+00:00"
        }],
        "history": [
            {
                "seq": 1, "kind": "claimed", "item": "task-one", "holder": "agent-a",
                "at": "2031-05-04T12:00:00+00:00", "epoch": 1, "previous_holder": null
            },
            {
                "seq": 2, "kind": "offered", "item": "task-one", "holder": null,
                "at": "2031-05-04T12:00:30+00:00", "epoch": 0, "previous_holder": null
            }
        ]
    });
    std::fs::write(
        dir.join("registry.json"),
        serde_json::to_string(&document).unwrap(),
    )
    .unwrap();
    let registry = Registry::new(dir.join("registry.json"));
    let error = expect_error(registry.board(&t0()));
    assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
    assert!(
        error
            .message()
            .contains("does not stand first in its history"),
        "{}",
        error.message()
    );
    assert!(!error.message().contains("twice"), "{}", error.message());
}

#[test]
fn cross_validation_accepts_a_history_that_opens_mid_life() {
    // The reader has always taken a standing item on its own
    // fields, so a recorded history may open in the middle of
    // a life. The join opens the chain at the first recorded
    // event, and the writers extend such a history one event
    // at a time, and every step still reads clean.
    let dir = temp_dir("cross-mid-life");
    let claimed = serde_json::json!({
        "format": "dibs-registry/1",
        "items": [{
            "id": "task-one", "note": "", "status": "claimed", "holder": "agent-a",
            "token_hash": dibs::token_hash("mid-life-token"), "epoch": 1,
            "expires_at": "2031-05-04T12:01:00+00:00"
        }],
        "history": [{
            "seq": 1, "kind": "claimed", "item": "task-one", "holder": "agent-a",
            "at": "2031-05-04T12:00:00+00:00", "epoch": 1, "previous_holder": null
        }]
    });
    std::fs::write(
        dir.join("registry.json"),
        serde_json::to_string(&claimed).unwrap(),
    )
    .unwrap();
    let registry = Registry::new(dir.join("registry.json"));
    let rows = registry.board(&t0()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "claimed");
    assert_eq!(rows[0].epoch, 1);
    registry
        .renew("task-one", "agent-a", "mid-life-token", 1, 60, &at(1))
        .unwrap();
    registry
        .release("task-one", "agent-a", "mid-life-token", 1, &at(2))
        .unwrap();
    let rows = registry.board(&at(3)).unwrap();
    assert_eq!(rows[0].status, "open");
    assert_eq!(rows[0].epoch, 1);

    // A chain that opens with a claim and a release already
    // recorded reads clean as it stands, and a fresh claim
    // advances the epoch from the recorded standing epoch.
    let dir = temp_dir("cross-mid-life-released");
    let released = serde_json::json!({
        "format": "dibs-registry/1",
        "items": [{
            "id": "task-two", "note": "", "status": "open", "holder": null,
            "token_hash": null, "epoch": 1, "expires_at": null
        }],
        "history": [
            {
                "seq": 1, "kind": "claimed", "item": "task-two", "holder": "agent-a",
                "at": "2031-05-04T12:00:00+00:00", "epoch": 1, "previous_holder": null
            },
            {
                "seq": 2, "kind": "released", "item": "task-two", "holder": "agent-a",
                "at": "2031-05-04T12:00:30+00:00", "epoch": 1, "previous_holder": null
            }
        ]
    });
    std::fs::write(
        dir.join("registry.json"),
        serde_json::to_string(&released).unwrap(),
    )
    .unwrap();
    let registry = Registry::new(dir.join("registry.json"));
    let granted = registry.claim("task-two", "agent-b", 60, &at(1)).unwrap();
    assert_eq!(granted.epoch, 2);
    let rows = registry.board(&at(2)).unwrap();
    assert_eq!(rows[0].status, "claimed");
    assert_eq!(rows[0].epoch, 2);

    // A history may also open with the takeover itself, at
    // the epoch the takeover assigned, which never stands
    // below 2 because the lease it took over was a claim and
    // a first claim carries epoch 1.
    let dir = temp_dir("cross-mid-life-takeover");
    let taken = serde_json::json!({
        "format": "dibs-registry/1",
        "items": [{
            "id": "task-three", "note": "", "status": "claimed", "holder": "agent-b",
            "token_hash": dibs::token_hash("takeover-token"), "epoch": 2,
            "expires_at": "2031-05-04T12:01:00+00:00"
        }],
        "history": [{
            "seq": 1, "kind": "taken_over", "item": "task-three", "holder": "agent-b",
            "at": "2031-05-04T12:00:00+00:00", "epoch": 2, "previous_holder": "agent-a"
        }]
    });
    std::fs::write(
        dir.join("registry.json"),
        serde_json::to_string(&taken).unwrap(),
    )
    .unwrap();
    let registry = Registry::new(dir.join("registry.json"));
    let rows = registry.board(&t0()).unwrap();
    assert_eq!(rows[0].status, "claimed");
    assert_eq!(rows[0].epoch, 2);
    registry
        .renew("task-three", "agent-b", "takeover-token", 2, 60, &at(1))
        .unwrap();
    let rows = registry.board(&at(2)).unwrap();
    assert_eq!(rows[0].status, "claimed");
    assert_eq!(rows[0].epoch, 2);
}

#[test]
fn cross_validation_reads_back_every_written_step() {
    let dir = temp_dir("cross-round-trip");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    assert_eq!(registry.history(None).unwrap().len(), 1);
    let first = registry.claim("task-one", "agent-a", 60, &at(1)).unwrap();
    assert_eq!(registry.board(&at(2)).unwrap()[0].status, "claimed");
    registry
        .renew("task-one", "agent-a", &first.token, 1, 60, &at(2))
        .unwrap();
    registry
        .release("task-one", "agent-a", &first.token, 1, &at(3))
        .unwrap();
    assert_eq!(registry.board(&at(4)).unwrap()[0].status, "open");
    let second = registry.claim("task-one", "agent-b", 60, &at(5)).unwrap();
    assert_eq!(second.epoch, 2);
    // The second lease runs out at second 65, so the claim at
    // second 200 takes the item over.
    let third = registry.claim("task-one", "agent-c", 60, &at(200)).unwrap();
    assert_eq!(third.epoch, 3);
    registry
        .complete("task-one", "agent-c", &third.token, 3, &at(201))
        .unwrap();
    let events = registry.history(None).unwrap();
    assert_eq!(events.len(), 7);
    assert_eq!(events[5].kind, "taken_over");
    assert_eq!(events[5].previous_holder.as_deref(), Some("agent-b"));
    assert_eq!(registry.board(&at(202)).unwrap().len(), 0);
}

#[test]
fn registry_survives_a_reopen_between_operations() {
    let dir = temp_dir("reopen");
    let first = make_registry(&dir);
    first.offer("task-one", "a synthetic task", &t0()).unwrap();
    let granted = first.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    let second = make_registry(&dir);
    assert_eq!(
        second.board(&at(1)).unwrap()[0].holder.as_deref(),
        Some("agent-a")
    );
    second
        .release("task-one", "agent-a", &granted.token, 1, &at(2))
        .unwrap();
    assert_eq!(first.board(&at(3)).unwrap()[0].status, "open");
}

/// The worker half of the process race pins. The parent pins
/// reenter this test binary with DIBS_RACE_WORKER set, and the
/// worker claims once and leaves by exit code, 0 on a grant, 3
/// on a refusal, 4 on anything else, writing the granted token
/// to the result file the parent named. With the variable unset
/// the pin is a no-op.
#[test]
fn race_worker_entry() {
    if std::env::var("DIBS_RACE_WORKER").is_err() {
        return;
    }
    let read = |key: &str| std::env::var(key).unwrap_or_default();
    let registry = Registry::new(PathBuf::from(read("DIBS_RACE_REGISTRY")));
    let micros: i128 = read("DIBS_RACE_AT_MICROS").parse().unwrap_or(0);
    let now = Instant::from_epoch_micros(micros).unwrap();
    let code = match registry.claim(
        &read("DIBS_RACE_ITEM"),
        &read("DIBS_RACE_HOLDER"),
        600,
        &now,
    ) {
        Ok(granted) => {
            std::fs::write(read("DIBS_RACE_RESULT"), granted.token).unwrap();
            0
        }
        Err(error) => {
            // A pin that needs the refusal's words names an
            // error file, and the worker leaves the message
            // there before it leaves by exit code.
            if let Ok(path) = std::env::var("DIBS_RACE_ERROR") {
                let _ = std::fs::write(path, error.message());
            }
            match error {
                Error::Refusal(_) => 3,
                _ => 4,
            }
        }
    };
    std::process::exit(code);
}

fn race_claims(dir: &Path, mode: &str, count: usize) -> (Vec<i32>, PathBuf) {
    let registry_path = dir.join("registry.json");
    let at_micros = at(10).epoch_micros().unwrap().to_string();
    let mut children = Vec::new();
    for index in 0..count {
        let item = if mode == "same" {
            "task-shared".to_string()
        } else {
            format!("task-{index}")
        };
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("race_worker_entry")
            .arg("--nocapture")
            .env("DIBS_RACE_WORKER", "1")
            .env("DIBS_RACE_REGISTRY", &registry_path)
            .env("DIBS_RACE_ITEM", item)
            .env("DIBS_RACE_HOLDER", format!("agent-{index}"))
            .env("DIBS_RACE_AT_MICROS", &at_micros)
            .env("DIBS_RACE_RESULT", dir.join(format!("result-{index}.txt")))
            .spawn()
            .unwrap();
        children.push(child);
    }
    let codes: Vec<i32> = children
        .into_iter()
        .map(|mut child| child.wait().unwrap().code().unwrap_or(-1))
        .collect();
    (codes, registry_path)
}

#[test]
fn race_on_one_item_produces_exactly_one_winner() {
    let dir = temp_dir("race-same");
    let registry = make_registry(&dir);
    registry
        .offer("task-shared", "a synthetic task", &t0())
        .unwrap();
    let (codes, registry_path) = race_claims(&dir, "same", 8);
    assert_eq!(codes.iter().filter(|code| **code == 0).count(), 1);
    assert_eq!(codes.iter().filter(|code| **code == 3).count(), 7);
    let winner = codes.iter().position(|code| *code == 0).unwrap();
    let document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&registry_path).unwrap()).unwrap();
    let item = &document["items"][0];
    assert_eq!(item["holder"].as_str().unwrap(), format!("agent-{winner}"));
    assert_eq!(item["epoch"].as_i64().unwrap(), 1);
    let kinds: Vec<&str> = document["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["offered", "claimed"]);
    let token = std::fs::read_to_string(dir.join(format!("result-{winner}.txt"))).unwrap();
    registry
        .release(
            "task-shared",
            &format!("agent-{winner}"),
            &token,
            1,
            &at(20),
        )
        .unwrap();
}

#[test]
fn race_on_distinct_items_loses_no_updates() {
    let dir = temp_dir("race-distinct");
    let registry = make_registry(&dir);
    for index in 0..8 {
        registry
            .offer(&format!("task-{index}"), "a synthetic task", &t0())
            .unwrap();
    }
    let (codes, registry_path) = race_claims(&dir, "distinct", 8);
    assert_eq!(codes, vec![0; 8]);
    let document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&registry_path).unwrap()).unwrap();
    let items = document["items"].as_array().unwrap();
    assert_eq!(items.len(), 8);
    assert!(items
        .iter()
        .all(|item| item["status"].as_str().unwrap() == "claimed"));
    assert!(items
        .iter()
        .all(|item| item["epoch"].as_i64().unwrap() == 1));
    let seqs: Vec<i64> = document["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["seq"].as_i64().unwrap())
        .collect();
    assert_eq!(seqs, (1..=16).collect::<Vec<i64>>());
    let claimed = document["history"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["kind"].as_str().unwrap() == "claimed")
        .count();
    assert_eq!(claimed, 8);
    let rows = Registry::new(registry_path).board(&at(20)).unwrap();
    assert_eq!(rows.len(), 8);
    assert!(rows.iter().all(|row| row.status == "claimed"));
}

/// A staged registry for the lock identity pin: the target
/// item and the parent item stand first, then the filler
/// items that make one claim span a wide window, every item
/// offered in order.
fn staged_lock_document(fillers: usize) -> String {
    let mut ids: Vec<String> = vec!["task-target".to_string(), "task-parent".to_string()];
    for index in 0..fillers {
        ids.push(format!("item-{index:06}"));
    }
    common::offered_document(&ids)
}

// DB-S1 gates this POSIX open-file unlink race pin to `#[cfg(unix)]`
// because Unix permits unlinking a file while a descriptor holds it,
// exercising the `(dev, ino)` lock-identity verification. On Windows,
// `DB-16` omits `FILE_SHARE_DELETE` so the operating system refuses
// deletion at the handle layer, pinned by the `#[cfg(windows)]`
// counterpart `lock_file_cannot_be_deleted_while_claim_holds_it` below.
#[cfg(unix)]
#[test]
fn lock_file_deleted_mid_claim_aborts_the_claim() {
    let dir = temp_dir("lock-identity");
    let registry_path = dir.join("registry.json");
    std::fs::write(&registry_path, staged_lock_document(16_000)).unwrap();
    let lock_path = dir.join("registry.json.lock");
    let error_path = dir.join("worker-error.txt");
    let at_micros = at(10).epoch_micros().unwrap().to_string();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("race_worker_entry")
        .arg("--nocapture")
        .env("DIBS_RACE_WORKER", "1")
        .env("DIBS_RACE_REGISTRY", &registry_path)
        .env("DIBS_RACE_ITEM", "task-target")
        .env("DIBS_RACE_HOLDER", "agent-victim")
        .env("DIBS_RACE_AT_MICROS", &at_micros)
        .env("DIBS_RACE_RESULT", dir.join("result-victim.txt"))
        .env("DIBS_RACE_ERROR", &error_path)
        .spawn()
        .unwrap();
    // Wait until the worker stands inside its claim, then
    // delete the lock file it holds. A claim spanning sixteen
    // thousand items leaves a wide window between the lock and
    // the publish, and the deletion lands inside it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !lock_path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the worker never created the lock file"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::remove_file(&lock_path).unwrap();
    // A second claim locks a fresh file at the lock path and
    // proceeds while the first claim still holds the deleted
    // file, and the second claim must stand.
    let registry = Registry::new(registry_path.clone());
    let granted = registry
        .claim("task-parent", "agent-parent", 600, &at(10))
        .unwrap();
    assert_eq!(granted.epoch, 1);
    // The first claim aborts on the lock identity check before
    // it can publish, so its success can never be erased.
    let code = child.wait().unwrap().code().unwrap_or(-1);
    assert_eq!(code, 4, "the victim claim must abort, not land");
    let message = std::fs::read_to_string(&error_path).unwrap();
    assert!(
        message.contains("is not the file this command locked"),
        "{message}"
    );
    assert!(
        message.contains("the registry was left unchanged"),
        "{message}"
    );
    // The victim claim left no trace. The target stands open
    // at epoch 0, the history is the offers and the parent
    // claim alone, gapless, and the registry still works.
    let rows = registry.board(&at(20)).unwrap();
    let target = rows.iter().find(|row| row.id == "task-target").unwrap();
    assert_eq!(target.status, "open");
    assert_eq!(target.epoch, 0);
    let events = registry.history(None).unwrap();
    assert_eq!(events.len(), 16_003);
    assert!(events
        .iter()
        .all(|event| event.item != "task-target" || event.kind == "offered"));
    let seqs: Vec<i128> = events.iter().map(|event| event.seq).collect();
    assert_eq!(seqs, (1..=16_003).collect::<Vec<i128>>());
    let claimed = registry
        .claim("task-target", "agent-parent", 600, &at(30))
        .unwrap();
    assert_eq!(claimed.epoch, 1);
}

#[cfg(windows)]
#[test]
fn lock_file_cannot_be_deleted_while_claim_holds_it() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = temp_dir("lock-no-share-delete");
    let registry_path = dir.join("registry.json");
    std::fs::write(&registry_path, staged_lock_document(100)).unwrap();
    let lock_path = dir.join("registry.json.lock");
    // Open a preliminary exclusive lock allowing delete sharing on
    // this test handle alone so the worker blocks deterministically
    // after opening lock_path without FILE_SHARE_DELETE.
    let blocker = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004)
        .open(&lock_path)
        .unwrap();
    blocker.lock().unwrap();

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let worker_barrier = std::sync::Arc::clone(&barrier);
    let worker_registry_path = registry_path.clone();
    let worker = std::thread::spawn(move || {
        let worker_reg = Registry::new(worker_registry_path);
        worker_barrier.wait();
        worker_reg.claim("task-target", "agent-victim", 60, &at(10))
    });

    // Synchronize immediately before Registry::claim and allow the in-process
    // thread to open lock_path without any competing DELETE-access handle
    // before verifying Win32 ERROR_SHARING_VIOLATION (32).
    barrier.wait();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        match std::fs::OpenOptions::new()
            .access_mode(0x0001_0000)
            .share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004)
            .open(&lock_path)
        {
            Ok(probe) => drop(probe),
            Err(err) if err.raw_os_error() == Some(32) => break,
            Err(err) => panic!("unexpected error probing lock_path delete access: {err:?}"),
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worker never opened lock_path without FILE_SHARE_DELETE"
        );
    }

    let del_err = std::fs::remove_file(&lock_path).unwrap_err();
    assert_eq!(del_err.raw_os_error(), Some(32));
    assert!(lock_path.exists());

    blocker.unlock().unwrap();
    drop(blocker);
    let granted = worker.join().unwrap().unwrap();
    assert_eq!(granted.holder, "agent-victim");
    let registry = Registry::new(registry_path);
    let rows = registry.board(&at(20)).unwrap();
    let target = rows.iter().find(|row| row.id == "task-target").unwrap();
    assert_eq!(target.status, "claimed");
    assert_eq!(target.holder.as_deref(), Some("agent-victim"));
    assert_eq!(target.epoch, 1);
}

struct CliOutput {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_cli(args: &[&str], expect: i32) -> CliOutput {
    // DB-S2 scrubs DIBS_REGISTRY and DIBS_TOKEN so ambient shell
    // environment variables cannot perturb standing CLI tests.
    let output = Command::new(env!("CARGO_BIN_EXE_dibs"))
        .env_remove("DIBS_REGISTRY")
        .env_remove("DIBS_TOKEN")
        .args(args)
        .output()
        .unwrap();
    let result = CliOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    };
    assert_eq!(result.code, expect, "stderr was: {}", result.stderr);
    result
}

#[test]
fn cli_round_trip_and_exit_codes() {
    let dir = temp_dir("cli-round-trip");
    let path = dir.join("registry.json");
    let path = path.to_str().unwrap();
    let offered = run_cli(
        &[
            "--registry",
            path,
            "offer",
            "task-one",
            "--note",
            "synthetic",
        ],
        0,
    );
    assert_eq!(offered.stdout, "offered task-one\n");
    let empty = run_cli(&["--registry", path, "board"], 0);
    assert_eq!(empty.stdout, "task-one open epoch=0\n");
    let claimed = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-one",
            "--holder",
            "agent-a",
            "--ttl",
            "600",
        ],
        0,
    );
    let lines: Vec<&str> = claimed.stdout.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("token "));
    let token = lines[0].split_once(' ').unwrap().1;
    assert_eq!(lines[1], "epoch 1");
    // The claim's third line prints the expiry the claim
    // recorded, and the board then reports that same expiry
    // for the lease. The board's field is compared whole, so
    // a claim that printed a different expiry than it
    // recorded, an empty one, a truncated one, or no third
    // line at all fails here, while the token and epoch
    // lines still pass.
    let printed_expiry = lines[2].strip_prefix("expires_at ").unwrap();
    let board = run_cli(&["--registry", path, "board"], 0);
    assert!(board.stdout.contains("task-one claimed holder=agent-a"));
    let board_line = board
        .stdout
        .lines()
        .find(|line| line.starts_with("task-one claimed"))
        .unwrap();
    let board_expiry = board_line
        .split("expires_at=")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    assert_eq!(board_expiry, printed_expiry);
    assert!(board.stdout.contains("epoch=1"));
    assert!(!board.stdout.contains(token));
    let renewed = run_cli(
        &[
            "--registry",
            path,
            "renew",
            "task-one",
            "--holder",
            "agent-a",
            &format!("--token={token}"),
            "--epoch",
            "1",
            "--ttl",
            "60",
        ],
        0,
    );
    assert!(renewed.stdout.starts_with("expires_at "));
    let history = run_cli(&["--registry", path, "history"], 0);
    assert!(history.stdout.contains("1 offered item=task-one"));
    assert!(history
        .stdout
        .contains("2 claimed item=task-one holder=agent-a"));
    assert!(history
        .stdout
        .contains("3 renewed item=task-one holder=agent-a"));
    assert!(!history.stdout.contains(token));
    let refused = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-one",
            "--holder",
            "agent-b",
            "--ttl",
            "60",
        ],
        1,
    );
    assert!(refused.stderr.starts_with("error: "));
    assert!(refused.stderr.contains("agent-a"));
    assert_eq!(refused.stdout, "");
    let missing = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-ghost",
            "--holder",
            "agent-b",
            "--ttl",
            "60",
        ],
        1,
    );
    assert!(missing.stderr.starts_with("error: "));
    let bad_name = run_cli(&["--registry", path, "offer", "Bad Name"], 2);
    assert!(bad_name.stderr.starts_with("error: "));
    let bad_ttl = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-one",
            "--holder",
            "agent-a",
            "--ttl",
            "0",
        ],
        2,
    );
    assert!(bad_ttl.stderr.starts_with("error: "));
    let released = run_cli(
        &[
            "--registry",
            path,
            "release",
            "task-one",
            "--holder",
            "agent-a",
            &format!("--token={token}"),
            "--epoch",
            "1",
        ],
        0,
    );
    assert_eq!(released.stdout, "released task-one\n");
}

#[test]
fn cli_integer_arguments_take_the_python_grammar() {
    let dir = temp_dir("cli-unicode-int");
    let path = dir.join("registry.json");
    let path = path.to_str().unwrap();
    run_cli(&["--registry", path, "offer", "task-one"], 0);
    // Arabic-Indic digits carry the time to live.
    let claimed = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-one",
            "--holder",
            "agent-a",
            "--ttl",
            "٤٢",
        ],
        0,
    );
    let token = claimed
        .stdout
        .lines()
        .next()
        .unwrap()
        .split_once(' ')
        .unwrap()
        .1
        .to_string();
    // An em space wrapped Arabic-Indic epoch and a Devanagari
    // time to live carry the renew.
    let renewed = run_cli(
        &[
            "--registry",
            path,
            "renew",
            "task-one",
            "--holder",
            "agent-a",
            &format!("--token={token}"),
            "--epoch",
            "\u{2003}١\u{2003}",
            "--ttl",
            "३०",
        ],
        0,
    );
    assert!(renewed.stdout.starts_with("expires_at "));
    run_cli(
        &[
            "--registry",
            path,
            "release",
            "task-one",
            "--holder",
            "agent-a",
            &format!("--token={token}"),
            "--epoch",
            "1",
        ],
        0,
    );
    // Fullwidth digits wrapped in no break spaces carry the
    // second claim, and a Devanagari epoch its release.
    let claimed = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-one",
            "--holder",
            "agent-a",
            "--ttl",
            "\u{a0}４２\u{a0}",
        ],
        0,
    );
    let token = claimed
        .stdout
        .lines()
        .next()
        .unwrap()
        .split_once(' ')
        .unwrap()
        .1
        .to_string();
    run_cli(
        &[
            "--registry",
            path,
            "release",
            "task-one",
            "--holder",
            "agent-a",
            &format!("--token={token}"),
            "--epoch",
            "२",
        ],
        0,
    );
    // A negative time to live in Arabic-Indic digits is taken
    // as the option's value, not mistaken for an option, and
    // the library refuses it with the value rendered in ASCII
    // digits, as the Python surface refused it.
    let refused = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-one",
            "--holder",
            "agent-a",
            "--ttl",
            "-٤٢",
        ],
        2,
    );
    assert!(
        refused.stderr.contains("invalid time to live -42"),
        "{}",
        refused.stderr
    );
}

#[test]
fn cli_integer_arguments_refuse_the_separator_controls() {
    // The separator controls U+001C to U+001F stand in the
    // wider whitespace set str.strip takes, and a reader could
    // expect the integer grammar to trim them. The Python
    // integer conversion does not: probed against the runtime,
    // an integer wrapped in them is refused there, and the
    // command line refuses it the same way, at exit 2.
    let dir = temp_dir("cli-separator-int");
    let path = dir.join("registry.json");
    let path = path.to_str().unwrap();
    run_cli(&["--registry", path, "offer", "task-one"], 0);
    let refused = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-one",
            "--holder",
            "agent-a",
            "--ttl",
            "\u{1c}60\u{1f}",
        ],
        2,
    );
    assert!(
        refused.stderr.contains("invalid int value"),
        "{}",
        refused.stderr
    );
}

#[test]
fn cli_integer_arguments_refuse_digit_strings_past_the_conversion_limit() {
    // The Python integer conversion the command line mirrors
    // refuses a digit string past its limit of 4300 digits,
    // counted as the conversion counts them, leading zeros
    // included and underscores excluded, and the parser
    // reports the argument as an invalid int value at exit 2.
    // The converter as first built kept a digit string of any
    // length whole, so a time to live past the limit refused
    // as an invalid time to live, the library's sentence for a
    // value the conversion never produced, and an epoch past
    // the limit was presented as the widest 128 bit count and
    // refused as a possession refusal at exit 1, a malformed
    // invocation reading as a lease refusal. The pins below
    // fail on that converter for those reasons, and the
    // control at the limit itself fails on a converter that
    // refuses one digit early.
    let dir = temp_dir("cli-digit-limit");
    let path = dir.join("registry.json");
    let path = path.to_str().unwrap();
    run_cli(&["--registry", path, "offer", "task-one"], 0);
    let claimed = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-one",
            "--holder",
            "agent-a",
            "--ttl",
            "600",
        ],
        0,
    );
    let token = claimed
        .stdout
        .lines()
        .next()
        .unwrap()
        .split_once(' ')
        .unwrap()
        .1
        .to_string();
    let past = "1".repeat(4301);
    let refused = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-one",
            "--holder",
            "agent-a",
            "--ttl",
            &past,
        ],
        2,
    );
    assert!(
        refused.stderr.contains("invalid int value"),
        "{}",
        refused.stderr
    );
    assert!(
        !refused.stderr.contains("invalid time to live"),
        "{}",
        refused.stderr
    );
    let refused = run_cli(
        &[
            "--registry",
            path,
            "release",
            "task-one",
            "--holder",
            "agent-a",
            &format!("--token={token}"),
            "--epoch",
            &past,
        ],
        2,
    );
    assert!(
        refused.stderr.contains("invalid int value"),
        "{}",
        refused.stderr
    );
    assert!(
        !refused
            .stderr
            .contains("170141183460469231731687303715884105727"),
        "{}",
        refused.stderr
    );
    // The limit counts the digits as written, not the value
    // they name, so an epoch of zeros past the limit refuses
    // the same way, where the value zero itself converts.
    let zeros = "0".repeat(4301);
    let refused = run_cli(
        &[
            "--registry",
            path,
            "release",
            "task-one",
            "--holder",
            "agent-a",
            &format!("--token={token}"),
            "--epoch",
            &zeros,
        ],
        2,
    );
    assert!(
        refused.stderr.contains("invalid int value"),
        "{}",
        refused.stderr
    );
    // The control at the limit itself: a time to live of
    // exactly 4300 digits still converts whole and refuses as
    // an invalid time to live naming the number, so the
    // boundary stands at the conversion's limit and not below
    // it.
    let at_limit = "1".repeat(4300);
    let refused = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-two",
            "--holder",
            "agent-a",
            "--ttl",
            &at_limit,
        ],
        2,
    );
    assert!(
        refused.stderr.contains("invalid time to live"),
        "{}",
        refused.stderr
    );
    assert!(
        !refused.stderr.contains("invalid int value"),
        "{}",
        refused.stderr
    );
    // Nothing above touched the standing lease.
    let board = run_cli(&["--registry", path, "board"], 0);
    assert!(
        board.stdout.contains("task-one claimed holder=agent-a"),
        "{}",
        board.stdout
    );
}

#[test]
fn cli_integer_arguments_cover_every_digit_family() {
    // The integer grammar takes a decimal digit from any of the
    // families of the converter's table, and the pins above
    // exercise three of them. This pin exercises all of them,
    // one renew per family, with the epoch and the time to live
    // written in the family's own digits. The epoch must match
    // the standing epoch exactly, so a family the table holds
    // at the wrong offset refuses the renew and fails here, and
    // a family the table misses fails here as a parser error.
    // The bases stand as a second copy of the table, each the
    // codepoint of its family's digit zero.
    const FAMILY_BASES: [u32; 68] = [
        0x0030, 0x0660, 0x06f0, 0x07c0, 0x0966, 0x09e6, 0x0a66, 0x0ae6, 0x0b66, 0x0be6, 0x0c66,
        0x0ce6, 0x0d66, 0x0de6, 0x0e50, 0x0ed0, 0x0f20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946,
        0x19d0, 0x1a80, 0x1a90, 0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0,
        0xa9f0, 0xaa50, 0xabf0, 0xff10, 0x104a0, 0x10d30, 0x11066, 0x110f0, 0x11136, 0x111d0,
        0x112f0, 0x11450, 0x114d0, 0x11650, 0x116c0, 0x11730, 0x118e0, 0x11950, 0x11c50, 0x11d50,
        0x11da0, 0x11f50, 0x16a60, 0x16ac0, 0x16b50, 0x1d7ce, 0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6,
        0x1e140, 0x1e2f0, 0x1e4f0, 0x1e950, 0x1fbf0,
    ];
    let dir = temp_dir("cli-digit-families");
    let path = dir.join("registry.json");
    let path = path.to_str().unwrap();
    run_cli(&["--registry", path, "offer", "task-one"], 0);
    let claimed = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-one",
            "--holder",
            "agent-a",
            "--ttl",
            "600",
        ],
        0,
    );
    let token = claimed
        .stdout
        .lines()
        .next()
        .unwrap()
        .split_once(' ')
        .unwrap()
        .1
        .to_string();
    for base in FAMILY_BASES {
        let digit = |value: u32| char::from_u32(base + value).unwrap().to_string();
        let epoch = digit(1);
        let ttl = format!("{}{}", digit(3), digit(0));
        let renewed = run_cli(
            &[
                "--registry",
                path,
                "renew",
                "task-one",
                "--holder",
                "agent-a",
                &format!("--token={token}"),
                "--epoch",
                &epoch,
                "--ttl",
                &ttl,
            ],
            0,
        );
        assert!(
            renewed.stdout.starts_with("expires_at "),
            "the digit family at {base:#06x} did not carry the renew: {}",
            renewed.stderr
        );
    }
}

#[cfg(unix)]
#[test]
fn cli_non_unicode_arguments_are_parser_errors_not_panics() {
    use std::os::unix::ffi::OsStringExt;

    let dir = temp_dir("cli-non-unicode");
    let path = dir.join("registry.json");
    // An item name holding bytes that are not Unicode is
    // refused at exit 2 with a parser error, where the parser
    // as first built panicked collecting the arguments.
    let bad = std::ffi::OsString::from_vec(vec![0xff, 0xfe]);
    let output = Command::new(env!("CARGO_BIN_EXE_dibs"))
        .arg("--registry")
        .arg(&path)
        .arg("offer")
        .arg(&bad)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.contains("not valid Unicode text"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
    // The registry path is the one argument carried
    // unconverted, so a path holding the same bytes passes
    // parser conversion in both the spaced and equals forms.
    // DB-S1 guards `create_dir_all` on filesystems such as
    // macOS APFS that reject non UTF-8 directory names on
    // disk, asserting that both forms still carry the raw
    // OsString past argument parsing without panicking.
    let odd_dir = dir.join(std::ffi::OsString::from_vec(vec![0xff]));
    let odd_path = odd_dir.join("registry.json");
    let mut equals = std::ffi::OsString::from("--registry=");
    equals.push(odd_path.as_os_str());
    if std::fs::create_dir_all(&odd_dir).is_ok() {
        let output = Command::new(env!("CARGO_BIN_EXE_dibs"))
            .arg("--registry")
            .arg(&odd_path)
            .arg("offer")
            .arg("task-one")
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "stderr was: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "offered task-one\n"
        );
        let output = Command::new(env!("CARGO_BIN_EXE_dibs"))
            .arg(&equals)
            .arg("offer")
            .arg("task-two")
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "stderr was: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "offered task-two\n"
        );
    } else {
        for form in [
            vec![
                std::ffi::OsString::from("--registry"),
                odd_path.clone().into(),
            ],
            vec![equals],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_dibs"))
                .args(&form)
                .arg("board")
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            assert_eq!(output.status.code(), Some(0), "{stderr}");
            assert_eq!(output.stdout, b"");
            assert!(!stderr.contains("not valid Unicode text"), "{stderr}");
            assert!(!stderr.contains("panicked"), "{stderr}");
        }
        let offer_out = Command::new(env!("CARGO_BIN_EXE_dibs"))
            .arg("--registry")
            .arg(&odd_path)
            .arg("offer")
            .arg("task-one")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&offer_out.stderr).into_owned();
        assert_eq!(offer_out.status.code(), Some(1), "{stderr}");
        assert!(
            stderr.contains("the registry directory cannot be created"),
            "{stderr}"
        );
        assert!(!stderr.contains("not valid Unicode text"), "{stderr}");
        assert!(!stderr.contains("panicked"), "{stderr}");
    }
}

#[test]
fn cli_surface_carries_no_time_override_flag() {
    // DB-S4 (DB-P142): also drive --at alongside --now and assert neither
    // creates the registry file on disk.
    let help = run_cli(&["--help"], 0);
    assert!(!help.stdout.contains("--now"));
    assert!(!help.stdout.contains("--at"));
    let dir = temp_dir("cli-no-override");
    let reg_path = dir.join("registry.json");
    let path = reg_path.to_str().unwrap();
    run_cli(
        &[
            "--registry",
            path,
            "offer",
            "task-one",
            "--now",
            "2031-01-01",
        ],
        2,
    );
    assert!(!reg_path.exists());
    run_cli(
        &[
            "--registry",
            path,
            "offer",
            "task-one",
            "--at",
            "2031-01-01",
        ],
        2,
    );
    assert!(!reg_path.exists());
}

#[test]
fn cli_huge_ttl_prints_one_error_line_and_no_traceback() {
    let dir = temp_dir("cli-huge-ttl");
    let path = dir.join("registry.json");
    let path = path.to_str().unwrap();
    run_cli(&["--registry", path, "offer", "task-one"], 0);
    let result = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-one",
            "--holder",
            "agent-a",
            "--ttl",
            "1000000000000000000",
        ],
        2,
    );
    assert!(result.stderr.starts_with("error: "));
    assert!(!result.stderr.contains("Traceback"));
    assert_eq!(result.stderr.matches('\n').count(), 1);
    assert_eq!(result.stdout, "");
}

#[test]
fn cli_completed_item_leaves_the_board_but_keeps_its_history() {
    let dir = temp_dir("cli-completed");
    let path = dir.join("registry.json");
    let path = path.to_str().unwrap();
    run_cli(&["--registry", path, "offer", "task-one"], 0);
    let claimed = run_cli(
        &[
            "--registry",
            path,
            "claim",
            "task-one",
            "--holder",
            "agent-a",
            "--ttl",
            "600",
        ],
        0,
    );
    let token = claimed
        .stdout
        .lines()
        .next()
        .unwrap()
        .split_once(' ')
        .unwrap()
        .1;
    let done = run_cli(
        &[
            "--registry",
            path,
            "complete",
            "task-one",
            "--holder",
            "agent-a",
            &format!("--token={token}"),
            "--epoch",
            "1",
        ],
        0,
    );
    assert_eq!(done.stdout, "completed task-one\n");
    assert_eq!(run_cli(&["--registry", path, "board"], 0).stdout, "");
    let history = run_cli(&["--registry", path, "history", "task-one"], 0);
    assert!(history
        .stdout
        .contains("3 completed item=task-one holder=agent-a"));
}

#[test]
fn cli_history_names_the_previous_holder_on_a_takeover() {
    // The history line carries the previous holder on a
    // taken_over event alone, and the library pins assert the
    // event's field rather than this surface's rendering of
    // it. The takeover is staged through the library at
    // injected instants, because history evaluates no lease
    // state and the command line read of it crosses no wall
    // clock boundary.
    let dir = temp_dir("cli-takeover-line");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    registry.claim("task-one", "agent-a", 60, &at(0)).unwrap();
    registry.claim("task-one", "agent-b", 60, &at(61)).unwrap();
    let path = dir.join("registry.json");
    let path = path.to_str().unwrap();
    let history = run_cli(&["--registry", path, "history"], 0);
    let lines: Vec<&str> = history.stdout.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(
        lines[2],
        "3 taken_over item=task-one holder=agent-b \
         at=2031-05-04T12:01:01+00:00 epoch=2 previous_holder=agent-a"
    );
    assert!(!lines[0].contains("previous_holder"), "{}", lines[0]);
    assert!(!lines[1].contains("previous_holder"), "{}", lines[1]);
}

#[test]
fn damage_duplicate_key_message_names_the_whole_key() {
    // The duplicated key here carries the separator the reader's
    // own position suffix uses, so a recovery that cuts the error
    // text at the first separator names only the part of the key
    // standing before it. The message names the whole key, and
    // the assertions compare the complete message, so a recovery
    // that kept the reader's position suffix would fail them too.
    let dir = temp_dir("damage-duplicate-named");
    let path = dir.join("registry.json");
    let text = "{\"format\": \"dibs-registry/1\", \"items\": [{\"id\": \"task-one\", \
         \"note at line 5\": \"a\", \"note at line 5\": \"b\", \"note\": \"\", \
         \"status\": \"open\", \"holder\": null, \"token_hash\": null, \"epoch\": 0, \
         \"expires_at\": null}], \"history\": []}";
    std::fs::write(&path, text).unwrap();
    let registry = Registry::new(path);
    let error = expect_error(registry.board(&t0()));
    assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
    assert_eq!(
        error.message(),
        "the registry is corrupt, a key stands twice in the document, note at line 5"
    );
    // The control: an ordinary duplicated key names that key in
    // the same message, as it did before the fix.
    let dir = temp_dir("damage-duplicate-plain");
    let path = dir.join("registry.json");
    let text = "{\"format\": \"dibs-registry/1\", \"format\": \"dibs-registry/1\", \
         \"items\": [], \"history\": []}";
    std::fs::write(&path, text).unwrap();
    let registry = Registry::new(path);
    let error = expect_error(registry.board(&t0()));
    assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
    assert_eq!(
        error.message(),
        "the registry is corrupt, a key stands twice in the document, format"
    );
}

#[cfg(unix)]
#[test]
fn cli_board_with_an_unwritable_stdout_exits_1_without_a_panic() {
    // Command output goes through a checked writer (DB-A8), so
    // a stream that refuses the write ends the command quietly
    // at exit 1. /dev/full stands in for the closed pipe of the
    // finding's settling probe, because a pipe whose reader
    // closes early cannot be staged deterministically from
    // inside the suite, and /dev/full refuses the write the
    // same way. Where /dev/full does not stand the staged
    // failure cannot be produced and the pin does not run; on
    // the hosted gate, which is Linux, the device stands. On
    // the pre-fix binary the same drive dies with a panic
    // message at exit 101.
    // DB-S1 adds an unconditional closed-reader `std::io::pipe()`
    // arm so macOS (which lacks `/dev/full`) deterministically
    // exercises the `EPIPE` stdout write failure too, while keeping
    // the `/dev/full` check wherever `/dev/full` stands.
    use std::process::Stdio;

    let dir = temp_dir("cli-board-full");
    let registry = make_registry(&dir);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();

    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let pipe_out = Command::new(env!("CARGO_BIN_EXE_dibs"))
        .arg("--registry")
        .arg(dir.join("registry.json"))
        .arg("board")
        .stdout(Stdio::from(writer))
        .output()
        .unwrap();
    assert_eq!(pipe_out.status.code(), Some(1));
    let pipe_stderr = String::from_utf8_lossy(&pipe_out.stderr).into_owned();
    assert!(!pipe_stderr.contains("panicked"), "{pipe_stderr}");
    assert!(
        !pipe_stderr.contains("failed printing to stdout"),
        "{pipe_stderr}"
    );

    if let Ok(full) = std::fs::OpenOptions::new().write(true).open("/dev/full") {
        let output = Command::new(env!("CARGO_BIN_EXE_dibs"))
            .arg("--registry")
            .arg(dir.join("registry.json"))
            .arg("board")
            .stdout(Stdio::from(full))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(!stderr.contains("panicked"), "{stderr}");
        assert!(!stderr.contains("failed printing to stdout"), "{stderr}");
    }
}

#[cfg(unix)]
#[test]
fn cli_claim_with_an_unwritable_stdout_exits_1_and_the_lease_stands() {
    // The claim publishes under the lock before its token is
    // printed, so a stream that refuses the print leaves the
    // lease standing with its token never delivered. The
    // checked writer reports that failure as a quiet exit 1,
    // where the pre-fix binary panicked at exit 101 over the
    // standing lease. Where /dev/full does not stand the pin
    // does not run, as its sibling above records.
    // DB-S1 adds an unconditional closed-reader `std::io::pipe()`
    // arm so macOS exercises the post-publish `EPIPE` stdout write
    // failure and verifies the standing lease, alongside `/dev/full`
    // wherever that device stands.
    use std::process::Stdio;

    let dir = temp_dir("cli-claim-full");
    let path = dir.join("registry.json");
    let path_text = path.to_str().unwrap();
    run_cli(&["--registry", path_text, "offer", "task-pipe"], 0);
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let pipe_out = Command::new(env!("CARGO_BIN_EXE_dibs"))
        .arg("--registry")
        .arg(&path)
        .arg("claim")
        .arg("task-pipe")
        .arg("--holder")
        .arg("agent-a")
        .arg("--ttl")
        .arg("3600")
        .stdout(Stdio::from(writer))
        .output()
        .unwrap();
    assert_eq!(pipe_out.status.code(), Some(1));
    let pipe_stderr = String::from_utf8_lossy(&pipe_out.stderr).into_owned();
    assert!(!pipe_stderr.contains("panicked"), "{pipe_stderr}");
    let pipe_board = run_cli(&["--registry", path_text, "board"], 0);
    assert!(
        pipe_board
            .stdout
            .contains("task-pipe claimed holder=agent-a"),
        "{}",
        pipe_board.stdout
    );
    assert!(
        pipe_board.stdout.contains("epoch=1"),
        "{}",
        pipe_board.stdout
    );

    if let Ok(full) = std::fs::OpenOptions::new().write(true).open("/dev/full") {
        run_cli(&["--registry", path_text, "offer", "task-one"], 0);
        let output = Command::new(env!("CARGO_BIN_EXE_dibs"))
            .arg("--registry")
            .arg(&path)
            .arg("claim")
            .arg("task-one")
            .arg("--holder")
            .arg("agent-a")
            .arg("--ttl")
            .arg("3600")
            .stdout(Stdio::from(full))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(!stderr.contains("panicked"), "{stderr}");
        let board = run_cli(&["--registry", path_text, "board"], 0);
        assert!(
            board.stdout.contains("task-one claimed holder=agent-a"),
            "{}",
            board.stdout
        );
        assert!(board.stdout.contains("epoch=1"), "{}", board.stdout);
    }
}

#[cfg(unix)]
#[test]
fn cli_invalid_command_word_refuses_before_the_arguments_after_it_are_converted() {
    // The command word is judged where it is taken (DB-A49), so
    // an unlisted word refuses as an invalid choice whatever
    // follows it. Before the fix the tokens after the word were
    // converted to text first, and a token that was not Unicode
    // among them preempted the judgment, reporting the word
    // under the history usage with the Unicode message instead,
    // so the same invocation refused two different ways
    // depending on the bytes of a later argument.
    use std::os::unix::ffi::OsStringExt;

    let dir = temp_dir("cli-invalid-command");
    let path = dir.join("registry.json");
    let bad = std::ffi::OsString::from_vec(vec![0xff, 0xfe]);
    let output = Command::new(env!("CARGO_BIN_EXE_dibs"))
        .arg("--registry")
        .arg(&path)
        .arg("bogus")
        .arg(&bad)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // DB-S2 mechanical update: MAIN_USAGE gained `[-V] [-j]` and made
    // `[--registry REGISTRY]` optional when `DIBS_REGISTRY` is set
    // under Obligation DB-20.
    assert!(
        stderr.starts_with("usage: dibs [-h] [-V] [-j] [--registry REGISTRY]"),
        "{stderr}"
    );
    assert!(stderr.contains("invalid choice: 'bogus'"), "{stderr}");
    assert!(!stderr.contains("usage: dibs history"), "{stderr}");
    assert!(!stderr.contains("not valid Unicode text"), "{stderr}");
    // The control arm: a listed word takes the same non Unicode
    // argument to the standing Unicode refusal under its own
    // usage, the arm the sibling pin above holds.
    let output = Command::new(env!("CARGO_BIN_EXE_dibs"))
        .arg("--registry")
        .arg(&path)
        .arg("offer")
        .arg(&bad)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.starts_with("usage: dibs offer [-h]"), "{stderr}");
    assert!(stderr.contains("not valid Unicode text"), "{stderr}");
}

#[test]
fn registry_documents_without_a_byte_order_mark_read_in_each_encoding() {
    // The Python json module detected the encoding of the
    // bytes it was handed by the NUL pattern of the opening
    // bytes as well as by a byte order mark, so a registry
    // written in UTF-16 or UTF-32 without a mark read as the
    // document it encodes. The reader as it stood took only
    // the mark, and a markless document fell to the UTF-8
    // reading, whose decode succeeds on the NUL bytes and
    // whose parse then refuses the interleaved text, so every
    // encoding below refused whole as corruption before the
    // repair this pin holds. Each encoding must read its
    // board at both sides of the lease's expiry boundary, and
    // a markless UTF-16 document carrying a lone surrogate in
    // its note must still refuse as corruption with its bytes
    // standing, so a repair that decoded the detected
    // encodings loosely stands caught beside one that never
    // detected them, and a repair that detected only one
    // endianness fails on the other. The mark forms stand
    // held by the byte order mark pin above, and a repair
    // that ran the pattern detection ahead of the mark arms
    // would decode the UTF-32 big endian mark as a character
    // and fail there.
    let text = encodable_claimed_document();
    for (mark, strip) in [
        ("utf-16le", 2usize),
        ("utf-16be", 2),
        ("utf-32le", 4),
        ("utf-32be", 4),
    ] {
        let dir = temp_dir(&format!("no-bom-{mark}"));
        let path = dir.join("registry.json");
        let marked = encode_document(&text, mark);
        std::fs::write(&path, &marked[strip..]).unwrap();
        let registry = Registry::new(path);
        let live = registry.board(&at(30)).unwrap();
        assert_eq!(live.len(), 1, "{mark}");
        assert_eq!(live[0].status, "claimed", "{mark}");
        assert_eq!(
            live[0].expires_at.as_deref(),
            Some("2031-05-04T12:01:00+00:00"),
            "{mark}"
        );
        let expired = registry.board(&at(60)).unwrap();
        assert_eq!(expired[0].status, "open", "{mark}");
    }
    // A markless UTF-16 document carrying a lone surrogate in
    // its note refuses as corruption and leaves the registry
    // bytes standing, the strict decoding the mark arms use.
    let note_b = text.find("\"abc\"").unwrap() + 2;
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units[note_b] = 0xd800;
    let mut bytes = Vec::new();
    for unit in &units {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let dir = temp_dir("no-bom-lone-surrogate");
    let path = dir.join("registry.json");
    std::fs::write(&path, &bytes).unwrap();
    let registry = Registry::new(path.clone());
    assert_corrupt(registry.board(&at(1)));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}
#[test]
fn damage_duplicate_key_with_a_newline_names_it_escaped() {
    // A duplicated key carrying a newline names the key in its
    // escaped rendering, so the refusal stays one line. The
    // standing code printed the recovered key raw, and the
    // message carried the newline itself.
    let dir = temp_dir("damage-duplicate-newline");
    let path = dir.join("registry.json");
    let text = "{\"format\": \"dibs-registry/1\", \"items\": [{\"id\": \"task-one\", \
         \"note\\nline\": \"a\", \"note\\nline\": \"b\", \"note\": \"\", \
         \"status\": \"open\", \"holder\": null, \"token_hash\": null, \"epoch\": 0, \
         \"expires_at\": null}], \"history\": []}";
    std::fs::write(&path, text).unwrap();
    let registry = Registry::new(path);
    let error = expect_error(registry.board(&t0()));
    assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
    assert_eq!(
        error.message(),
        "the registry is corrupt, a key stands twice in the document, 'note\\nline'"
    );
    assert!(!error.message().contains('\n'));
}

#[test]
fn a_registry_whose_metadata_cannot_be_read_is_a_failure_not_an_empty_backlog() {
    // The registry path stands under a regular file, so its
    // metadata fails for every user, where the read path as
    // first built took any metadata failure for absence and
    // read the empty backlog (DB-A7). Board and history
    // refuse with the failure an unreadable registry raises.
    // The probe behind the finding staged the same refusal
    // through a directory that denies search permission; this
    // shape can fail on any box, whatever user runs the suite.
    let dir = temp_dir("metadata-denied-registry");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "a regular file").unwrap();
    let registry = Registry::new(blocker.join("registry.json"));
    let error = expect_error(registry.board(&t0()));
    assert!(matches!(error, Error::Failure(_)), "{error:?}");
    assert!(
        error.message().contains("the registry file cannot be read"),
        "{}",
        error.message()
    );
    let error = expect_error(registry.history(None));
    assert!(matches!(error, Error::Failure(_)), "{error:?}");
    // The control: a registry that does not stand reads as an
    // empty backlog and creates nothing, as DB-12 states.
    let missing = Registry::new(dir.join("missing.json"));
    assert_eq!(missing.board(&t0()).unwrap().len(), 0);
    assert_eq!(missing.history(None).unwrap().len(), 0);
    assert!(!dir.join("missing.json").exists());
    assert!(!dir.join("missing.json.lock").exists());
}

#[test]
fn string_values_render_in_refusal_messages_as_python_renders_them() {
    // The messages that interpolate a value render a string
    // the way Python's repr renders it, and that rendering is
    // an equivalence obligation of the port (DB-A56). The
    // float arms stand pinned by the sibling float pin, and
    // the private use escapes by the pin queued with DB-A5's
    // repair, but the renderer's quote and escape arms stood
    // unpinned before this pin. The cases stand on those
    // arms: the quote selection itself (a double quote when
    // the text carries a single quote and no double quote, a
    // single quote otherwise, the chosen quote escaped where
    // the text carries it), the backslash escape, the short
    // escape for a tab, and the hex escape widths, two
    // digits at or under a byte and four in the rest of the
    // basic plane, for the characters the printable table
    // refuses, closing with a printable character past the
    // basic plane rendering as itself. Every expected text
    // was probed against the Python runtime the contract
    // names, 3.12.3, never written from habit. A renderer
    // that always chose one quote fails the first three
    // cases, one that passed a control character through
    // raw fails the escape cases (and would break the one
    // line shape the refusal keeps), one that rendered a
    // byte width character at four digits fails the two
    // digit cases, and one that escaped a printable astral
    // character fails the last case, while every variant
    // only pin around them still passed.
    let cases: [(&str, &str); 10] = [
        ("task'one", "\"task'one\""),
        ("task\"one", "'task\"one'"),
        ("task'\"one", "'task\\'\"one'"),
        ("task\\one", "'task\\\\one'"),
        ("task\tone", "'task\\tone'"),
        ("task\u{1}one", "'task\\x01one'"),
        ("task\u{85}one", "'task\\x85one'"),
        ("task\u{a0}one", "'task\\xa0one'"),
        ("task\u{200b}one", "'task\\u200bone'"),
        ("task\u{20000}one", "'task\u{20000}one'"),
    ];
    for (name, rendered) in cases {
        let error = expect_error(validate_name(&Value::Str(name.to_string()), "item"));
        assert!(matches!(error, Error::InvalidName(_)), "{error:?}");
        let expected = format!(
            "invalid item id {rendered}, use one component of lowercase letters \
             and digits and hyphens and underscores, starting and ending \
             with a letter or a digit, at most 64 characters long"
        );
        assert_eq!(error.message(), expected.as_str(), "the name {name:?}");
    }
}

#[test]
fn compound_format_values_render_whole_in_the_format_refusal() {
    // The strict reader's format refusal interpolates the
    // standing format value whole (DB-A56), and a damaged
    // document can carry a compound value in that field,
    // which the renderer's array and object arms join
    // element by element. The expected texts were probed
    // against Python 3.12.3: a list renders its elements
    // repr by repr joined with ", ", and an object renders
    // each key's repr, ": ", and the value's repr the same
    // way. A renderer that joined a compound value in any
    // other shape, or rendered a member in any other form,
    // fails these comparisons while every variant only pin
    // around them still passed.
    // DB-S3 mechanical update (DB-21): the upward-compatible reader accepts
    // 'dibs-registry/1' and 'dibs-registry/2', so the format refusal suffix
    // names both versions.
    let base = temp_dir("format-compound");
    let (directory, text) = mutated(&base, "list-case", |doc| {
        doc["format"] = serde_json::json!(["dibs-registry/1", 2]);
    });
    let path = directory.join("registry.json");
    std::fs::write(&path, &text).unwrap();
    let registry = Registry::new(path);
    let error = expect_error(registry.board(&at(1)));
    assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
    assert_eq!(
        error.message(),
        "the registry is corrupt, the format name is ['dibs-registry/1', 2], \
         not 'dibs-registry/1' or 'dibs-registry/2'"
    );
    let (directory, text) = mutated(&base, "object-case", |doc| {
        doc["format"] = serde_json::json!({"name": "dibs-registry/1", "version": 2});
    });
    let path = directory.join("registry.json");
    std::fs::write(&path, &text).unwrap();
    let registry = Registry::new(path);
    let error = expect_error(registry.board(&at(1)));
    assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
    assert_eq!(
        error.message(),
        "the registry is corrupt, the format name is \
         {'name': 'dibs-registry/1', 'version': 2}, not 'dibs-registry/1' or 'dibs-registry/2'"
    );
}

#[test]
fn possession_check_judges_holder_then_epoch_then_token() {
    // The possession check judges holder first and epoch
    // second and token last, as its doc comment states, and
    // the order is observable only where two presentations
    // fail at once (DB-A57). Every standing pin isolates
    // one failure, so a refactor that swapped two of the
    // three checks would pass the whole suite while
    // changing which refusal a doubly wrong presentation
    // draws. These three arms present two failures at a
    // time on the standing claimed item shape the suite
    // hand builds, comparing the message whole, and a swap
    // of any two checks fails exactly one arm. The last
    // arm stands as the control, the same text the binding
    // smoke pins on its surface.
    let standing = Item {
        id: "task-one".to_string(),
        note: "a synthetic task".to_string(),
        status: "claimed".to_string(),
        holder: Some("agent-a".to_string()),
        token_hash: Some(dibs::token_hash("tok")),
        epoch: 1,
        expires_at: Some(dibs::format_instant(&at(60))),
    };
    let error = expect_error(check_possession(
        &standing,
        "agent-b",
        "tok-wrong",
        &Value::Int(7),
    ));
    assert!(matches!(error, Error::Refusal(_)), "{error:?}");
    assert_eq!(
        error.message(),
        "item 'task-one' is held by 'agent-a', not by 'agent-b'"
    );
    let error = expect_error(check_possession(
        &standing,
        "agent-a",
        "tok-wrong",
        &Value::Int(7),
    ));
    assert!(matches!(error, Error::Refusal(_)), "{error:?}");
    assert_eq!(
        error.message(),
        "the epoch 7 presented for item 'task-one' does not match \
         the standing epoch 1"
    );
    let error = expect_error(check_possession(
        &standing,
        "agent-a",
        "tok-wrong",
        &Value::Int(1),
    ));
    assert!(matches!(error, Error::Refusal(_)), "{error:?}");
    assert_eq!(
        error.message(),
        "the token presented for item 'task-one' does not match the lease"
    );
}

#[test]
fn cli_help_bodies_print_their_whole_texts() {
    // The help bodies stand pinned as whole texts.
    // DB-S2 mechanical update: main help and subcommand help bodies
    // gained `-V, --version`, `-j, --json`, flexible `--registry`
    // placement (`DIBS_REGISTRY` fallback), `--token` stdin/env
    // wording on lease-guarded subcommands, and the four DB-S2
    // subcommands (`offer-batch`, `claim-next`, `show`, `completions`)
    // under Obligations DB-17 through DB-20.
    // DB-S3 mechanical update (DB-20, DB-22, DB-23, DB-24): main help and
    // subcommand help bodies gained `reset`, `ready`, and `compact` plus the
    // `/2` options on `offer` and `claim-next`.
    let main = run_cli(&["--help"], 0);
    assert_eq!(main.stderr, "");
    assert_eq!(
        main.stdout,
        concat!(
            "usage: dibs [-h] [-V] [-j] [--registry REGISTRY]\n",
            "            {offer,offer-batch,claim,claim-next,renew,release,complete,reset,board,ready,show,history,compact,completions} ...\n",
            "\n",
            "Claim tasks from a shared backlog under leases that expire unless they are\n",
            "renewed.\n",
            "\n",
            "positional arguments:\n",
            "  {offer,offer-batch,claim,claim-next,renew,release,complete,reset,board,ready,show,history,compact,completions}\n",
            "    offer               add an item to the backlog\n",
            "    offer-batch         add a batch of items from JSON Lines input\n",
            "    claim               lease an item under a new claim\n",
            "    claim-next          lease the first ready item in the backlog\n",
            "    renew               extend the standing lease from its current expiry\n",
            "    release             end the standing lease early\n",
            "    complete            mark the item done under its standing lease\n",
            "    reset               reset an exhausted item back to open\n",
            "    board               show the open and claimed items\n",
            "    ready               show ready items ordered by priority and offer order\n",
            "    show                show the standing detail of one item\n",
            "    history             show the recorded claim history\n",
            "    compact             archive history events through a sequence number\n",
            "    completions         print shell completion script\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -V, --version         show program version and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog, or DIBS_REGISTRY\n",
            "                        when omitted, created by the first mutation\n",
        )
    );
    let offer = run_cli(&["offer", "--help"], 0);
    assert_eq!(offer.stderr, "");
    assert_eq!(
        offer.stdout,
        concat!(
            "usage: dibs offer [-h] [-j] [--registry REGISTRY] [--note NOTE]\n",
            "                  [--priority PRIORITY] [--tag TAG] [--depends-on ITEM]\n",
            "                  [--max-takeovers MAX_TAKEOVERS] item\n",
            "\n",
            "positional arguments:\n",
            "  item\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --note NOTE           a short payload note for the item\n",
            "  --priority PRIORITY   task priority from 0 critical to 3 low\n",
            "  --tag TAG             repeatable capability tag for the item\n",
            "  --depends-on ITEM     repeatable prerequisite item id\n",
            "  --max-takeovers MAX_TAKEOVERS\n",
            "                        maximum takeovers before item becomes exhausted\n",
        )
    );
    let offer_batch = run_cli(&["offer-batch", "--help"], 0);
    assert_eq!(offer_batch.stderr, "");
    assert_eq!(
        offer_batch.stdout,
        concat!(
            "usage: dibs offer-batch [-h] [-j] [--registry REGISTRY] [--file FILE]\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --file FILE           JSON Lines batch file, or - for standard input\n",
        )
    );
    let claim = run_cli(&["claim", "--help"], 0);
    assert_eq!(claim.stderr, "");
    assert_eq!(
        claim.stdout,
        concat!(
            "usage: dibs claim [-h] [-j] [--registry REGISTRY] --holder HOLDER --ttl TTL\n",
            "                  item\n",
            "\n",
            "positional arguments:\n",
            "  item\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --holder HOLDER\n",
            "  --ttl TTL             the lease length in seconds\n",
        )
    );
    let claim_next = run_cli(&["claim-next", "--help"], 0);
    assert_eq!(claim_next.stderr, "");
    assert_eq!(
        claim_next.stdout,
        concat!(
            "usage: dibs claim-next [-h] [-j] [--registry REGISTRY] --holder HOLDER\n",
            "                       --ttl TTL [--tag TAG]\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --holder HOLDER\n",
            "  --ttl TTL             the lease length in seconds\n",
            "  --tag TAG             repeatable required capability tag\n",
        )
    );
    let renew = run_cli(&["renew", "--help"], 0);
    assert_eq!(renew.stderr, "");
    assert_eq!(
        renew.stdout,
        concat!(
            "usage: dibs renew [-h] [-j] [--registry REGISTRY] --holder HOLDER\n",
            "                  [--token TOKEN] --epoch EPOCH --ttl TTL item\n",
            "\n",
            "positional arguments:\n",
            "  item\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --holder HOLDER\n",
            "  --token TOKEN         lease bearer token, - for stdin, or DIBS_TOKEN\n",
            "  --epoch EPOCH\n",
            "  --ttl TTL             the added lease length in seconds\n",
        )
    );
    let release = run_cli(&["release", "--help"], 0);
    assert_eq!(release.stderr, "");
    assert_eq!(
        release.stdout,
        concat!(
            "usage: dibs release [-h] [-j] [--registry REGISTRY] --holder HOLDER\n",
            "                    [--token TOKEN] --epoch EPOCH item\n",
            "\n",
            "positional arguments:\n",
            "  item\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --holder HOLDER\n",
            "  --token TOKEN         lease bearer token, - for stdin, or DIBS_TOKEN\n",
            "  --epoch EPOCH\n",
        )
    );
    let complete = run_cli(&["complete", "--help"], 0);
    assert_eq!(complete.stderr, "");
    assert_eq!(
        complete.stdout,
        concat!(
            "usage: dibs complete [-h] [-j] [--registry REGISTRY] --holder HOLDER\n",
            "                     [--token TOKEN] --epoch EPOCH item\n",
            "\n",
            "positional arguments:\n",
            "  item\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --holder HOLDER\n",
            "  --token TOKEN         lease bearer token, - for stdin, or DIBS_TOKEN\n",
            "  --epoch EPOCH\n",
        )
    );
    let reset = run_cli(&["reset", "--help"], 0);
    assert_eq!(reset.stderr, "");
    assert_eq!(
        reset.stdout,
        concat!(
            "usage: dibs reset [-h] [-j] [--registry REGISTRY] item\n",
            "\n",
            "positional arguments:\n",
            "  item\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
        )
    );
    let board = run_cli(&["board", "--help"], 0);
    assert_eq!(board.stderr, "");
    assert_eq!(
        board.stdout,
        concat!(
            "usage: dibs board [-h] [-j] [--registry REGISTRY]\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
        )
    );
    let ready = run_cli(&["ready", "--help"], 0);
    assert_eq!(ready.stderr, "");
    assert_eq!(
        ready.stdout,
        concat!(
            "usage: dibs ready [-h] [-j] [--registry REGISTRY] [--tag TAG]\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --tag TAG             repeatable required capability tag\n",
        )
    );
    let show = run_cli(&["show", "--help"], 0);
    assert_eq!(show.stderr, "");
    assert_eq!(
        show.stdout,
        concat!(
            "usage: dibs show [-h] [-j] [--registry REGISTRY] item\n",
            "\n",
            "positional arguments:\n",
            "  item\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
        )
    );
    let history = run_cli(&["history", "--help"], 0);
    assert_eq!(history.stderr, "");
    assert_eq!(
        history.stdout,
        concat!(
            "usage: dibs history [-h] [-j] [--registry REGISTRY] [item]\n",
            "\n",
            "positional arguments:\n",
            "  item\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
        )
    );
    let compact = run_cli(&["compact", "--help"], 0);
    assert_eq!(compact.stderr, "");
    assert_eq!(
        compact.stdout,
        concat!(
            "usage: dibs compact [-h] [-j] [--registry REGISTRY] --through-seq THROUGH_SEQ\n",
            "                    --archive ARCHIVE\n",
            "\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --through-seq THROUGH_SEQ\n",
            "                        inclusive sequence number to compact through\n",
            "  --archive ARCHIVE     JSON Lines archive file path\n",
        )
    );
    let completions = run_cli(&["completions", "--help"], 0);
    assert_eq!(completions.stderr, "");
    assert_eq!(
        completions.stdout,
        concat!(
            "usage: dibs completions [-h] {bash,zsh,fish}\n",
            "\n",
            "positional arguments:\n",
            "  {bash,zsh,fish}\n",
            "\n",
            "options:\n",
            "  -h, --help  show this help message and exit\n",
        )
    );
}

#[test]
fn invalid_registry_paths_refuse_at_exit_2_without_touching_disk() {
    let dir = temp_dir("invalid-registry-paths");
    #[cfg(not(windows))]
    let static_bad = ["", ".", "..", "./.", "/", "sub/", "sub/.", "sub/.."];
    #[cfg(windows)]
    let static_bad = [
        "",
        ".",
        "..",
        "./.",
        "/",
        "\\",
        "C:",
        "C:\\",
        "sub/",
        "sub/.",
        "sub/..",
        "sub\\",
        "sub\\.",
        "sub\\..",
        "registry.json.",
        "registry.json ",
        "sub\\registry.json.",
        "sub\\registry.json ",
    ];
    let anchored_bad = format!("{}/", dir.join("nested").display());
    let mut all_bad: Vec<String> = static_bad.iter().map(|s| (*s).to_string()).collect();
    all_bad.push(anchored_bad);

    let archive_candidate = dir.join("valid-archive.jsonl");
    let archive_str = archive_candidate.to_str().unwrap();
    for raw in &all_bad {
        let expected = format!(
            "invalid registry path {}, the path must name a file",
            dibs::py_repr(raw)
        );
        let registry = Registry::new(PathBuf::from(raw));
        // DB-S2 and DB-S3 extend DB-15 invalid-registry-path coverage to
        // offer_many, claim_next, show, board_details, ready, reset, and
        // compact alongside the standing operations.
        for error in [
            expect_error(registry.board(&t0())),
            expect_error(registry.board_details(&t0())),
            expect_error(registry.ready(&[], &t0())),
            expect_error(registry.show("task-one", &t0())),
            expect_error(registry.history(None)),
            expect_error(registry.offer("task-one", "", &t0())),
            expect_error(registry.offer_many(&[OfferSpec::new("task-one", "")], &t0())),
            expect_error(registry.claim("task-one", "agent-a", 60, &t0())),
            expect_error(registry.claim_next("agent-a", 60, &t0())),
            expect_error(registry.renew("task-one", "agent-a", "tok", 1, 60, &t0())),
            expect_error(registry.release("task-one", "agent-a", "tok", 1, &t0())),
            expect_error(registry.complete("task-one", "agent-a", "tok", 1, &t0())),
            expect_error(registry.reset("task-one", &t0())),
            expect_error(registry.compact(1, &archive_candidate, &t0())),
        ] {
            assert!(
                matches!(error, Error::InvalidRequest(_)),
                "path {raw:?} returned {error:?}"
            );
            assert_eq!(error.exit_code(), 2);
            assert!(error.message().starts_with("invalid registry path "));
            assert!(error.message().ends_with(", the path must name a file"));
            assert_eq!(error.message(), expected);
        }

        for subcmd in [
            vec!["--registry", raw.as_str(), "board"],
            vec!["board", "--registry", raw.as_str()],
            vec!["--registry", raw.as_str(), "ready"],
            vec!["--registry", raw.as_str(), "show", "task-one"],
            vec!["--registry", raw.as_str(), "history"],
            vec!["--registry", raw.as_str(), "offer", "task-one"],
            vec!["--registry", raw.as_str(), "reset", "task-one"],
            vec![
                "--registry",
                raw.as_str(),
                "compact",
                "--through-seq",
                "1",
                "--archive",
                archive_str,
            ],
            vec![
                "--registry",
                raw.as_str(),
                "offer-batch",
                "--file",
                "missing-batch.jsonl",
            ],
            vec![
                "--registry",
                raw.as_str(),
                "claim-next",
                "--holder",
                "agent-a",
                "--ttl",
                "60",
            ],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_dibs"))
                .env_remove("DIBS_REGISTRY")
                .env_remove("DIBS_TOKEN")
                .current_dir(&dir)
                .args(&subcmd)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            assert_eq!(
                output.status.code(),
                Some(2),
                "args {subcmd:?} stderr: {stderr}"
            );
            assert_eq!(stderr, format!("error: {expected}\n"));
        }
    }
    // Zero directories, files, or .lock artifacts were created in `dir`
    // by either the CLI runs or the `dir`-anchored library mutations.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);

    // An existing named directory without a trailing separator still
    // fails at read/open as Error::Failure at exit 1.
    let named_dir = dir.join("existing-dir");
    std::fs::create_dir_all(&named_dir).unwrap();
    let named_registry = Registry::new(named_dir);
    let error = expect_error(named_registry.board(&t0()));
    assert!(matches!(error, Error::Failure(_)), "{error:?}");
    assert_eq!(error.exit_code(), 1);
    assert!(error.message().contains("the registry file cannot be read"));

    // Positive dot-adjacent filename edges must succeed rather than
    // being misclassified as `.` or `..` directory syntax.
    #[cfg(unix)]
    let valid_dot_edges = [".registry", "..registry", "registry.", "registry.."];
    #[cfg(not(unix))]
    let valid_dot_edges = [".registry", "..registry"];
    for (idx, valid_name) in valid_dot_edges.iter().enumerate() {
        let edge_path = dir.join(format!("edge-{idx}")).join(valid_name);
        let edge_registry = Registry::new(edge_path.clone());
        edge_registry
            .offer("task-one", "dot-adjacent filename", &t0())
            .unwrap();
        let rows = edge_registry.board(&t0()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "task-one");
        assert!(
            edge_path.is_file(),
            "expected file at {}",
            edge_path.display()
        );
    }
}

#[test]
fn board_and_history_never_create_a_lock_file_on_disk() {
    let dir = temp_dir("read-never-creates-lock");
    let path = dir.join("registry.json");
    let lock_path = dir.join("registry.json.lock");
    let registry = Registry::new(path.clone());
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    assert!(lock_path.is_file());
    std::fs::remove_file(&lock_path).unwrap();
    assert!(!lock_path.exists());

    let rows = registry.board(&t0()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "task-one");
    assert!(!lock_path.exists());

    let events = registry.history(None).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "offered");
    assert!(!lock_path.exists());

    let path_str = path.to_str().unwrap();
    let cli_board = run_cli(&["--registry", path_str, "board"], 0);
    assert_eq!(cli_board.stdout, "task-one open epoch=0\n");
    assert!(!lock_path.exists());
    let cli_history = run_cli(&["--registry", path_str, "history"], 0);
    assert!(cli_history.stdout.contains("1 offered item=task-one"));
    assert!(!lock_path.exists());

    // When a lock file does stand, holding a shared read lock blocks
    // an exclusive try_lock while still permitting concurrent reads.
    registry
        .offer("task-two", "a second synthetic task", &at(1))
        .unwrap();
    assert!(lock_path.is_file());
    let mut reader_opts = std::fs::OpenOptions::new();
    reader_opts.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        reader_opts.share_mode(0x0000_0001 | 0x0000_0002);
    }
    let held_shared = reader_opts.open(&lock_path).unwrap();
    held_shared.lock_shared().unwrap();

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let del_probe = std::fs::OpenOptions::new()
            .access_mode(0x0001_0000)
            .share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004)
            .open(&lock_path)
            .unwrap_err();
        assert_eq!(del_probe.raw_os_error(), Some(32));
        let del_err = std::fs::remove_file(&lock_path).unwrap_err();
        assert_eq!(del_err.raw_os_error(), Some(32));
    }

    let mut writer_opts = std::fs::OpenOptions::new();
    writer_opts.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        writer_opts.share_mode(0x0000_0001 | 0x0000_0002);
    }
    let writer_probe = writer_opts.open(&lock_path).unwrap();
    assert!(
        writer_probe.try_lock().is_err(),
        "shared read lock must block exclusive try_lock"
    );
    assert_eq!(registry.board(&at(2)).unwrap().len(), 2);
    assert_eq!(registry.ready(&[], &at(2)).unwrap().len(), 2);
    assert_eq!(registry.show("task-one", &at(2)).unwrap().id, "task-one");
    assert_eq!(registry.history(None).unwrap().len(), 2);
    held_shared.unlock().unwrap();
    drop(held_shared);
    writer_probe.try_lock().unwrap();
    writer_probe.unlock().unwrap();
    drop(writer_probe);

    // Discriminating concurrency pin: while an exclusive lock is held on
    // `registry.json.lock` over transient invalid bytes, `board`, `ready`
    // (added in DB-S3 under DB-16/DB-22), `show` (added in DB-S2 under DB-19),
    // and `history` must block on `lock_shared()` rather than proceeding
    // lock-free (`Ok(None)`), reading only the valid bytes published before
    // the exclusive lock releases.
    let valid_bytes = std::fs::read(&path).unwrap();
    let mut blocker_opts = std::fs::OpenOptions::new();
    blocker_opts.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        blocker_opts.share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004);
    }
    let blocker = blocker_opts.open(&lock_path).unwrap();
    blocker.lock().unwrap();
    std::fs::write(&path, b"transient pre-publish").unwrap();

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
    let b_board = std::sync::Arc::clone(&barrier);
    let p_board = path.clone();
    let board_handle = std::thread::spawn(move || {
        b_board.wait();
        Registry::new(p_board).board(&at(2))
    });
    let b_ready = std::sync::Arc::clone(&barrier);
    let p_ready = path.clone();
    let ready_handle = std::thread::spawn(move || {
        b_ready.wait();
        Registry::new(p_ready).ready(&[], &at(2))
    });
    let b_show = std::sync::Arc::clone(&barrier);
    let p_show = path.clone();
    let show_handle = std::thread::spawn(move || {
        b_show.wait();
        Registry::new(p_show).show("task-one", &at(2))
    });
    let b_hist = std::sync::Arc::clone(&barrier);
    let p_hist = path.clone();
    let history_handle = std::thread::spawn(move || {
        b_hist.wait();
        Registry::new(p_hist).history(None)
    });

    barrier.wait();
    std::thread::sleep(std::time::Duration::from_millis(30));
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let del_probe = std::fs::OpenOptions::new()
            .access_mode(0x0001_0000)
            .share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004)
            .open(&lock_path)
            .unwrap_err();
        assert_eq!(del_probe.raw_os_error(), Some(32));
        let del_err = std::fs::remove_file(&lock_path).unwrap_err();
        assert_eq!(del_err.raw_os_error(), Some(32));
    }
    std::fs::write(&path, &valid_bytes).unwrap();
    blocker.unlock().unwrap();
    drop(blocker);

    let board_rows = board_handle.join().unwrap().unwrap();
    let ready_details = ready_handle.join().unwrap().unwrap();
    let show_detail = show_handle.join().unwrap().unwrap();
    let history_events = history_handle.join().unwrap().unwrap();
    assert_eq!(board_rows.len(), 2);
    assert_eq!(ready_details.len(), 2);
    assert_eq!(show_detail.id, "task-one");
    assert_eq!(history_events.len(), 2);
}

#[test]
fn retry_transient_rename_retries_permission_denied_and_sharing_violation_up_to_five_times() {
    // 5 transient PermissionDenied errors followed by Ok succeeds on call 6.
    let mut calls = 0usize;
    dibs::retry_transient_rename(|| {
        calls += 1;
        if calls <= 5 {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "transient access denied",
            ))
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(calls, 6);

    // 5 transient Win32 ERROR_SHARING_VIOLATION (os error 32) errors
    // followed by Ok succeeds on call 6.
    let mut calls = 0usize;
    dibs::retry_transient_rename(|| {
        calls += 1;
        if calls <= 5 {
            Err(std::io::Error::from_raw_os_error(32))
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(calls, 6);

    // A 6th consecutive sharing violation exhausts the 5-retry bound and fails.
    let mut calls = 0usize;
    let err = dibs::retry_transient_rename(|| {
        calls += 1;
        Err(std::io::Error::from_raw_os_error(32))
    })
    .unwrap_err();
    assert_eq!(calls, 6);
    assert_eq!(err.raw_os_error(), Some(32));

    // A non-transient error (NotFound) fails immediately on attempt 1.
    let mut calls = 0usize;
    let err = dibs::retry_transient_rename(|| {
        calls += 1;
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing temp file",
        ))
    })
    .unwrap_err();
    assert_eq!(calls, 1);
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[cfg(unix)]
#[test]
fn board_and_history_succeed_inside_a_read_only_directory() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("readonly-dir-reads");
    let ro_dir = dir.join("ro");
    std::fs::create_dir_all(&ro_dir).unwrap();
    let path = ro_dir.join("registry.json");
    let lock_path = ro_dir.join("registry.json.lock");
    let registry = Registry::new(path);
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
    std::fs::remove_file(&lock_path).unwrap();

    // Case 1: mode 0555 directory with no .lock file standing on disk.
    // DB-S2 extends read-only directory verification to Registry::show (DB-19).
    std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let rows = registry.board(&t0()).unwrap();
    let detail = registry.show("task-one", &t0()).unwrap();
    let events = registry.history(None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "task-one");
    assert_eq!(detail.id, "task-one");
    assert_eq!(events.len(), 1);
    assert!(!lock_path.exists());

    // Case 2: mode 0555 directory with a readable .lock file standing on disk.
    std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(&lock_path, b"").unwrap();
    std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o444)).unwrap();
    std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    assert_eq!(registry.board(&t0()).unwrap().len(), 1);
    assert_eq!(registry.show("task-one", &t0()).unwrap().id, "task-one");
    assert_eq!(registry.history(None).unwrap().len(), 1);

    // Case 3: unreadable .lock file (0000) falls back lock-free over the registry.
    std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(registry.board(&t0()).unwrap().len(), 1);
    assert_eq!(registry.show("task-one", &t0()).unwrap().id, "task-one");
    assert_eq!(registry.history(None).unwrap().len(), 1);

    // Restore permissions so temp directory cleanup always succeeds.
    std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o644)).unwrap();
}

#[cfg(unix)]
#[test]
fn exclusive_lock_creation_honors_a_group_writable_umask() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("umask-group-lock");
    let path = dir.join("registry.json");
    let lock_path = dir.join("registry.json.lock");
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg("umask 0002 && exec \"$1\" --registry \"$2\" offer task-one")
        .arg("sh")
        .arg(env!("CARGO_BIN_EXE_dibs"))
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mode = std::fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o664,
        "expected group-writable 0o664 under umask 0o002, got {mode:04o}"
    );
}

fn run_cli_stdin_env(
    args: &[&str],
    stdin_bytes: Option<&[u8]>,
    envs: &[(&str, Option<&str>)],
    expect: i32,
) -> CliOutput {
    use std::io::Write;
    use std::process::Stdio;

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dibs"));
    cmd.env_remove("DIBS_REGISTRY").env_remove("DIBS_TOKEN");
    for (key, val) in envs {
        match val {
            Some(v) => {
                cmd.env(key, v);
            }
            None => {
                cmd.env_remove(key);
            }
        }
    }
    cmd.args(args);
    if stdin_bytes.is_some() {
        cmd.stdin(Stdio::piped());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    if let Some(bytes) = stdin_bytes {
        if let Some(mut handle) = child.stdin.take() {
            let _ = handle.write_all(bytes);
        }
    }
    let output = child.wait_with_output().unwrap();
    let result = CliOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    };
    assert_eq!(
        result.code, expect,
        "args {args:?} stdout: {} stderr: {}",
        result.stdout, result.stderr
    );
    result
}

fn object_keys(val: &Value) -> Vec<&str> {
    let Value::Object(pairs) = val else {
        panic!("expected JSON object, got {val:?}");
    };
    pairs.iter().map(|(k, _)| k.as_str()).collect()
}

fn object_field<'a>(val: &'a Value, key: &str) -> &'a Value {
    let Value::Object(pairs) = val else {
        panic!("expected JSON object, got {val:?}");
    };
    pairs
        .iter()
        .find_map(|(k, v)| if k == key { Some(v) } else { None })
        .unwrap_or_else(|| panic!("missing key {key} in {val:?}"))
}

#[test]
fn claim_next_selects_first_claimable_takes_over_expired_and_refuses_empty() {
    let dir = temp_dir("claim-next-lifecycle");

    // Missing registry under non-existent parent refuses cleanly at exit 1
    // without creating directories, registry files, or lock files.
    let missing_path = dir.join("deep").join("missing.json");
    let missing_reg = Registry::new(missing_path.clone());
    let err = expect_error(missing_reg.claim_next("agent-a", 60, &t0()));
    assert!(matches!(err, Error::Refusal(_)), "{err:?}");
    assert_eq!(err.exit_code(), 1);
    assert_eq!(err.message(), "no ready item stands in the backlog");
    assert!(!dir.join("deep").exists());
    assert!(!missing_path.exists());

    // Missing registry inside an existing directory also creates neither
    // registry file nor .lock file.
    let existing_missing = dir.join("missing.json");
    let existing_missing_reg = Registry::new(existing_missing.clone());
    let err_existing = expect_error(existing_missing_reg.claim_next("agent-a", 60, &t0()));
    assert!(
        matches!(err_existing, Error::Refusal(_)),
        "{err_existing:?}"
    );
    assert_eq!(err_existing.exit_code(), 1);
    assert_eq!(
        err_existing.message(),
        "no ready item stands in the backlog"
    );
    assert!(!existing_missing.exists());
    assert!(!dir.join("missing.json.lock").exists());

    // Epoch fence: when the first open item stands at u64::MAX, claim_next
    // refuses at exit 1 rather than overflowing or skipping item 0.
    let top_path = dir.join("epoch-top.json");
    let top_doc = "{\"format\": \"dibs-registry/1\", \"items\": [\
        {\"id\": \"task-top\", \"note\": \"\", \"status\": \"open\", \"holder\": null, \
         \"token_hash\": null, \"epoch\": 18446744073709551615, \"expires_at\": null}, \
        {\"id\": \"task-next\", \"note\": \"\", \"status\": \"open\", \"holder\": null, \
         \"token_hash\": null, \"epoch\": 0, \"expires_at\": null}], \"history\": []}";
    std::fs::write(&top_path, top_doc).unwrap();
    let top_reg = Registry::new(top_path.clone());
    let top_err = expect_error(top_reg.claim_next("agent-a", 60, &t0()));
    assert!(matches!(top_err, Error::Refusal(_)), "{top_err:?}");
    assert_eq!(top_err.exit_code(), 1);
    assert!(top_err.message().contains("18446744073709551615"));
    assert!(top_err
        .message()
        .contains("no epoch past it can be assigned"));
    let top_cli = run_cli_stdin_env(
        &[
            "--registry",
            top_path.to_str().unwrap(),
            "claim-next",
            "--holder",
            "agent-a",
            "--ttl",
            "60",
        ],
        None,
        &[],
        1,
    );
    assert!(top_cli.stderr.contains("18446744073709551615"));
    assert_eq!(std::fs::read(&top_path).unwrap(), top_doc.as_bytes());

    // Invalid inputs and invalid paths refuse at exit 2 before touching disk.
    assert_invalid_name(missing_reg.claim_next("Bad Holder", 60, &t0()));
    assert_invalid_ttl(missing_reg.claim_next("agent-a", 0, &t0()));
    let late = Instant::from_civil(9999, 12, 31, 23, 59, 0, 0, Some(0)).unwrap();
    assert_invalid_ttl(missing_reg.claim_next("agent-a", 3600, &late));
    let bad_path_err =
        expect_error(Registry::new(PathBuf::from("")).claim_next("agent-a", 60, &t0()));
    assert!(matches!(bad_path_err, Error::InvalidRequest(_)));
    assert_eq!(bad_path_err.exit_code(), 2);

    // Succeeding human-mode CLI `dibs claim-next` emits all 4 lines and matches board & show.
    let human_path = dir.join("human-next.json");
    let human_str = human_path.to_str().unwrap();
    run_cli_stdin_env(
        &[
            "--registry",
            human_str,
            "offer",
            "task-human",
            "--note",
            "human check",
        ],
        None,
        &[],
        0,
    );
    let human_claim = run_cli_stdin_env(
        &[
            "--registry",
            human_str,
            "claim-next",
            "--holder",
            "agent-h",
            "--ttl",
            "600",
        ],
        None,
        &[],
        0,
    );
    let claim_lines: Vec<&str> = human_claim.stdout.lines().collect();
    assert_eq!(claim_lines.len(), 4, "{}", human_claim.stdout);
    assert_eq!(claim_lines[0], "item task-human");
    let human_tok = claim_lines[1].strip_prefix("token ").unwrap();
    assert_eq!(human_tok.len(), 43);
    assert_eq!(claim_lines[2], "epoch 1");
    let human_exp = claim_lines[3].strip_prefix("expires_at ").unwrap();
    let check_board = run_cli_stdin_env(&["--registry", human_str, "board"], None, &[], 0);
    assert_eq!(
        check_board.stdout,
        format!("task-human claimed holder=agent-h expires_at={human_exp} epoch=1\n")
    );
    let check_show = run_cli_stdin_env(
        &["--registry", human_str, "show", "task-human"],
        None,
        &[],
        0,
    );
    assert!(check_show
        .stdout
        .contains(&format!("expires_at {human_exp}\n")));

    // Selection skips live claimed and completed items and picks first open item.
    let path = dir.join("registry.json");
    let registry = Registry::new(path.clone());
    registry
        .offer_many(
            &[
                OfferSpec::new("task-a", "first"),
                OfferSpec::new("task-b", "second"),
                OfferSpec::new("task-c", "third"),
            ],
            &t0(),
        )
        .unwrap();

    let first = registry.claim_next("agent-a", 60, &at(0)).unwrap();
    assert_eq!(first.item, "task-a");
    assert_eq!(first.holder, "agent-a");
    assert_eq!(first.epoch, 1);
    assert_eq!(first.expires_at, at(60));

    let second = registry.claim_next("agent-b", 60, &at(1)).unwrap();
    assert_eq!(second.item, "task-b");
    assert_eq!(second.epoch, 1);
    registry
        .complete("task-b", "agent-b", &second.token, 1, &at(2))
        .unwrap();

    // At t+10, task-a is live-claimed and task-b is completed, so claim_next
    // selects task-c.
    let third = registry.claim_next("agent-c", 60, &at(10)).unwrap();
    assert_eq!(third.item, "task-c");
    assert_eq!(third.epoch, 1);

    // At t+20, zero claimable items stand; refusal leaves registry byte-identical.
    let before = std::fs::read(&path).unwrap();
    let exhausted = expect_error(registry.claim_next("agent-d", 60, &at(20)));
    assert!(matches!(exhausted, Error::Refusal(_)), "{exhausted:?}");
    assert_eq!(exhausted.message(), "no ready item stands in the backlog");
    assert_eq!(std::fs::read(&path).unwrap(), before);

    // At exact expiry boundary t+60 of task-a, claim_next takes over task-a
    // at epoch 2 and records taken_over naming previous_holder agent-a.
    let takeover = registry.claim_next("agent-d", 60, &at(60)).unwrap();
    assert_eq!(takeover.item, "task-a");
    assert_eq!(takeover.holder, "agent-d");
    assert_eq!(takeover.epoch, 2);
    assert_ne!(takeover.token, first.token);

    let events = registry.history(Some("task-a")).unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.kind, "taken_over");
    assert_eq!(last.holder.as_deref(), Some("agent-d"));
    assert_eq!(last.previous_holder.as_deref(), Some("agent-a"));
    assert_eq!(last.epoch, 2);
}

#[test]
fn claim_next_sixteen_way_process_race_has_sixteen_first_try_winners() {
    let dir = temp_dir("claim-next-16-race");
    let path = dir.join("registry.json");
    let registry = Registry::new(path.clone());
    let specs: Vec<OfferSpec> = (0..16)
        .map(|idx| OfferSpec::new(format!("item-{idx:02}"), format!("payload {idx}")))
        .collect();
    registry.offer_many(&specs, &t0()).unwrap();

    // Spawn all 16 OS processes concurrently before waiting on any of them.
    let mut spawned = Vec::with_capacity(16);
    for idx in 0..16 {
        spawned.push(
            Command::new(env!("CARGO_BIN_EXE_dibs"))
                .env_remove("DIBS_REGISTRY")
                .env_remove("DIBS_TOKEN")
                .arg("--registry")
                .arg(&path)
                .arg("claim-next")
                .arg("--holder")
                .arg(format!("agent-{idx:02}"))
                .arg("--ttl")
                .arg("600")
                .arg("--json")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }

    let mut claimed_items = std::collections::BTreeSet::new();
    let mut tokens = std::collections::BTreeSet::new();
    for (idx, child) in spawned.into_iter().enumerate() {
        let output = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(0),
            "worker {idx} failed on first try: {stderr}"
        );
        assert_eq!(stderr, "");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(
            object_keys(&parsed),
            vec!["item", "holder", "token", "epoch", "expires_at"]
        );
        let Value::Str(item_id) = object_field(&parsed, "item") else {
            panic!("expected string item");
        };
        let Value::Str(tok) = object_field(&parsed, "token") else {
            panic!("expected string token");
        };
        assert_eq!(tok.len(), 43);
        assert_eq!(object_field(&parsed, "epoch"), &Value::Int(1));
        assert!(
            claimed_items.insert(item_id.clone()),
            "duplicate winner on {item_id}"
        );
        assert!(tokens.insert(tok.clone()), "duplicate token issued");
    }
    let expected_items: std::collections::BTreeSet<String> =
        (0..16).map(|idx| format!("item-{idx:02}")).collect();
    assert_eq!(claimed_items, expected_items);

    // 17th claim-next exits 1 cleanly and leaves registry bytes untouched.
    let before = std::fs::read(&path).unwrap();
    let path_str = path.to_str().unwrap();
    let seventeenth = run_cli_stdin_env(
        &[
            "--registry",
            path_str,
            "claim-next",
            "--holder",
            "agent-16",
            "--ttl",
            "600",
        ],
        None,
        &[],
        1,
    );
    assert_eq!(seventeenth.stdout, "");
    assert_eq!(
        seventeenth.stderr,
        "error: no ready item stands in the backlog\n"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);

    let events = registry.history(None).unwrap();
    assert_eq!(events.len(), 32);
    let seqs: Vec<i128> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=32).collect::<Vec<i128>>());
}

#[test]
fn offer_many_and_cli_offer_batch_enforce_all_or_nothing_atomicity() {
    let dir = temp_dir("offer-batch-atomicity");
    let path = dir.join("registry.json");
    let path_str = path.to_str().unwrap();
    let registry = Registry::new(path.clone());

    // Empty batch and intra-batch duplicate refuse at exit 2 without touching disk.
    let empty_err = expect_error(registry.offer_many(&[], &t0()));
    assert!(matches!(empty_err, Error::InvalidRequest(_)));
    assert_eq!(empty_err.exit_code(), 2);
    assert!(!path.exists());

    let dup_err = expect_error(registry.offer_many(
        &[
            OfferSpec::new("task-one", "a"),
            OfferSpec::new("task-two", "b"),
            OfferSpec::new("task-one", "c"),
        ],
        &t0(),
    ));
    assert!(matches!(dup_err, Error::InvalidRequest(_)));
    assert_eq!(dup_err.exit_code(), 2);
    assert!(!path.exists());

    let bad_note_err = expect_error(registry.offer_many(
        &[
            OfferSpec::new("task-one", "ok"),
            OfferSpec::new("task-two", "x".repeat(201)),
        ],
        &t0(),
    ));
    assert!(matches!(bad_note_err, Error::InvalidRequest(_)));
    assert_eq!(bad_note_err.exit_code(), 2);
    assert!(!path.exists());

    // Invalid item name and naive / out-of-range Instant refuse at exit 2 without touching disk.
    let bad_name_err = expect_error(registry.offer_many(
        &[
            OfferSpec::new("task-one", "ok"),
            OfferSpec::new("Bad Name", "ok"),
        ],
        &t0(),
    ));
    assert!(matches!(bad_name_err, Error::InvalidName(_)));
    assert_eq!(bad_name_err.exit_code(), 2);
    assert!(!path.exists());

    let naive = Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, None).unwrap();
    let early = Instant::from_civil(1, 1, 1, 0, 0, 0, 0, Some(3_600_000_000)).unwrap();
    for bad_now in [naive, early] {
        let now_err =
            expect_error(registry.offer_many(&[OfferSpec::new("task-one", "ok")], &bad_now));
        assert!(matches!(now_err, Error::InvalidRequest(_)));
        assert_eq!(now_err.exit_code(), 2);
        assert!(!path.exists());
    }

    // Valid batch via --file on CLI writes dibs-registry/1 format cleanly.
    let batch_file = dir.join("batch.jsonl");
    std::fs::write(
        &batch_file,
        "\n  \n{\"id\": \"task-one\", \"note\": \"first note\"}\n{\"id\": \"task-two\"}\n",
    )
    .unwrap();
    let out = run_cli_stdin_env(
        &[
            "--registry",
            path_str,
            "offer-batch",
            "--file",
            batch_file.to_str().unwrap(),
        ],
        None,
        &[],
        0,
    );
    assert_eq!(out.stdout, "offered 2\n");

    let before = std::fs::read(&path).unwrap();
    let raw_doc: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(raw_doc["format"], "dibs-registry/1");

    // Standing duplicate on item 3 of 5 refuses the ENTIRE batch at exit 1,
    // leaving items 1 and 2 unwritten and registry bytes untouched.
    let standing_dup = run_cli_stdin_env(
        &["--registry", path_str, "offer-batch"],
        Some(
            concat!(
                "{\"id\": \"fresh-1\"}\n",
                "{\"id\": \"fresh-2\"}\n",
                "{\"id\": \"task-two\"}\n",
                "{\"id\": \"fresh-4\"}\n",
                "{\"id\": \"fresh-5\"}\n",
            )
            .as_bytes(),
        ),
        &[],
        1,
    );
    assert_eq!(standing_dup.stdout, "");
    assert_eq!(
        standing_dup.stderr,
        "error: item 'task-two' already stands in the backlog\n"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(registry.board(&t0()).unwrap().len(), 2);

    // Malformed JSONL shapes all refuse at exit 2 leaving registry untouched.
    for bad_jsonl in [
        "",
        "   \n\t\n",
        "{not json}\n",
        "[1, 2]\n",
        "{\"note\": \"missing id\"}\n",
        "{\"id\": \"fresh-1\", \"unknown\": 1}\n",
        "{\"id\": \"fresh-1\", \"id\": \"fresh-2\"}\n",
        "{\"id\": \"fresh-1\"}\n{\"id\": \"fresh-1\"}\n",
        "{\"id\": \"Bad Name\"}\n",
    ] {
        let res = run_cli_stdin_env(
            &["--registry", path_str, "offer-batch", "--file", "-"],
            Some(bad_jsonl.as_bytes()),
            &[],
            2,
        );
        assert_eq!(res.stdout, "");
        assert!(res.stderr.starts_with("error: "), "{}", res.stderr);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    // Valid stdin batch with --json appends consecutively.
    let stdin_ok = run_cli_stdin_env(
        &["--registry", path_str, "offer-batch", "--json"],
        Some(b"{\"id\": \"task-three\", \"note\": \"third\"}\n{\"id\": \"task-four\"}\n"),
        &[],
        0,
    );
    assert_eq!(
        stdin_ok.stdout,
        "{\"count\": 2, \"offered\": [\"task-three\", \"task-four\"]}\n"
    );
    assert_eq!(registry.board(&t0()).unwrap().len(), 4);
    assert_eq!(registry.history(None).unwrap().len(), 4);
}

#[test]
fn show_evaluates_lazy_expiry_takeovers_and_never_creates_lock_file() {
    let dir = temp_dir("show-lazy-expiry");
    let path = dir.join("registry.json");
    let lock_path = dir.join("registry.json.lock");
    let path_str = path.to_str().unwrap();
    let registry = Registry::new(path.clone());

    // Missing registry refuses with NotFound at exit 1 and creates no lock file.
    let missing_err = expect_error(registry.show("task-one", &t0()));
    assert!(matches!(missing_err, Error::NotFound(_)));
    assert_eq!(missing_err.exit_code(), 1);
    assert_eq!(
        missing_err.message(),
        "item 'task-one' does not stand in the backlog"
    );
    assert!(!lock_path.exists());

    assert_invalid_name(registry.show("Bad Name", &t0()));
    let bad_reg_err = expect_error(Registry::new(PathBuf::from("sub/")).show("task-one", &t0()));
    assert!(matches!(bad_reg_err, Error::InvalidRequest(_)));
    assert_eq!(bad_reg_err.exit_code(), 2);

    // Naive and out-of-UTC-range instants refuse at exit 2 on show and board_details.
    let naive = Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, None).unwrap();
    let early = Instant::from_civil(1, 1, 1, 0, 0, 0, 0, Some(3_600_000_000)).unwrap();
    for bad_now in [naive, early] {
        let show_err = expect_error(registry.show("task-one", &bad_now));
        assert!(matches!(show_err, Error::InvalidRequest(_)));
        assert_eq!(show_err.exit_code(), 2);
        let details_err = expect_error(registry.board_details(&bad_now));
        assert!(matches!(details_err, Error::InvalidRequest(_)));
        assert_eq!(details_err.exit_code(), 2);
    }

    registry
        .offer("task-one", "line one\r\nline two", &t0())
        .unwrap();
    // Claim with a multi-hour TTL from wall clock so CLI `show` sees it live,
    // and also verify deterministic injected instants via library `show`.
    let cli_claim = run_cli_stdin_env(
        &[
            "--registry",
            path_str,
            "claim",
            "task-one",
            "--holder",
            "agent-a",
            "--ttl",
            "3600",
            "--json",
        ],
        None,
        &[],
        0,
    );
    let claim_val: Value = serde_json::from_str(cli_claim.stdout.trim()).unwrap();
    let Value::Str(tok1) = object_field(&claim_val, "token").clone() else {
        panic!("expected token");
    };
    let Value::Str(exp1) = object_field(&claim_val, "expires_at").clone() else {
        panic!("expected expires_at");
    };
    let exp1_inst = dibs::parse_instant(&exp1).unwrap();
    let just_before = Instant::from_epoch_micros(exp1_inst.epoch_micros().unwrap() - 1).unwrap();

    // Remove lock file and verify show never recreates it.
    std::fs::remove_file(&lock_path).unwrap();
    let live = registry.show("task-one", &just_before).unwrap();
    assert!(!lock_path.exists());
    assert_eq!(live.id, "task-one");
    assert_eq!(live.note, "line one\r\nline two");
    assert_eq!(live.status, "claimed");
    assert_eq!(live.effective_status, "claimed");
    assert_eq!(live.priority, 2);
    assert!(live.tags.is_empty());
    assert!(live.depends_on.is_empty());
    assert_eq!(live.holder.as_deref(), Some("agent-a"));
    assert_eq!(live.expires_at.as_deref(), Some(exp1.as_str()));
    assert_eq!(live.epoch, 1);
    assert_eq!(live.takeover_count, 0);
    assert_eq!(live.max_takeovers, None);

    // Human CLI show in live-claimed state prints all 12 lines verbatim
    // (escaping both \r and \n in note) and creates no lock file.
    let cli_human_live =
        run_cli_stdin_env(&["--registry", path_str, "show", "task-one"], None, &[], 0);
    assert!(!lock_path.exists());
    assert_eq!(
        cli_human_live.stdout,
        format!(
            concat!(
                "id task-one\n",
                "note line one\\r\\nline two\n",
                "status claimed\n",
                "effective_status claimed\n",
                "priority 2\n",
                "tags -\n",
                "depends_on -\n",
                "holder agent-a\n",
                "expires_at {}\n",
                "epoch 1\n",
                "takeover_count 0\n",
                "max_takeovers -\n",
            ),
            exp1
        )
    );

    // Lazy expiry at exact boundary exp1_inst: status & effective_status become open,
    // holder & expires_at clear to None.
    let expired = registry.show("task-one", &exp1_inst).unwrap();
    assert_eq!(expired.status, "open");
    assert_eq!(expired.effective_status, "open");
    assert_eq!(expired.holder, None);
    assert_eq!(expired.expires_at, None);
    assert_eq!(expired.epoch, 1);
    assert_eq!(expired.takeover_count, 0);

    // Takeover increments takeover_count to 1, and completion renders `-` for
    // null holder and expires_at across all 12 human lines.
    let second = registry
        .claim("task-one", "agent-b", 60, &exp1_inst)
        .unwrap();
    assert_ne!(second.token, tok1);
    let after_takeover = registry
        .show("task-one", &exp1_inst.plus_seconds(1))
        .unwrap();
    assert_eq!(after_takeover.status, "claimed");
    assert_eq!(after_takeover.effective_status, "claimed");
    assert_eq!(after_takeover.holder.as_deref(), Some("agent-b"));
    assert_eq!(after_takeover.epoch, 2);
    assert_eq!(after_takeover.takeover_count, 1);

    registry
        .complete(
            "task-one",
            "agent-b",
            &second.token,
            2,
            &exp1_inst.plus_seconds(2),
        )
        .unwrap();
    let completed = registry
        .show("task-one", &exp1_inst.plus_seconds(3))
        .unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.effective_status, "completed");
    assert_eq!(completed.holder, None);
    assert_eq!(completed.expires_at, None);
    assert_eq!(completed.epoch, 2);
    assert_eq!(completed.takeover_count, 1);

    let cli_human_done =
        run_cli_stdin_env(&["--registry", path_str, "show", "task-one"], None, &[], 0);
    assert_eq!(
        cli_human_done.stdout,
        concat!(
            "id task-one\n",
            "note line one\\r\\nline two\n",
            "status completed\n",
            "effective_status completed\n",
            "priority 2\n",
            "tags -\n",
            "depends_on -\n",
            "holder -\n",
            "expires_at -\n",
            "epoch 2\n",
            "takeover_count 1\n",
            "max_takeovers -\n",
        )
    );
}

#[test]
fn cli_json_mode_emits_exact_ordered_keys_and_never_leaks_token_hash() {
    let dir = temp_dir("cli-json-all-subcommands");
    let path = dir.join("registry.json");
    let path_str = path.to_str().unwrap();

    // 1. offer (--json before subcommand)
    let offer_res = run_cli_stdin_env(
        &[
            "--json",
            "--registry",
            path_str,
            "offer",
            "task-one",
            "--note",
            "alpha",
        ],
        None,
        &[],
        0,
    );
    let offer_val: Value = serde_json::from_str(offer_res.stdout.trim()).unwrap();
    assert_eq!(object_keys(&offer_val), vec!["item", "status", "epoch"]);
    assert_eq!(
        offer_res.stdout,
        "{\"item\": \"task-one\", \"status\": \"open\", \"epoch\": 0}\n"
    );

    // 2. offer-batch (-j after subcommand)
    let batch_res = run_cli_stdin_env(
        &["--registry", path_str, "offer-batch", "-j"],
        Some(b"{\"id\": \"task-two\", \"note\": \"beta\"}\n{\"id\": \"task-three\"}\n"),
        &[],
        0,
    );
    let batch_val: Value = serde_json::from_str(batch_res.stdout.trim()).unwrap();
    assert_eq!(object_keys(&batch_val), vec!["count", "offered"]);

    // 3. claim (--json after subcommand)
    let claim_res = run_cli_stdin_env(
        &[
            "--registry",
            path_str,
            "claim",
            "task-one",
            "--holder",
            "alice",
            "--ttl",
            "600",
            "--json",
        ],
        None,
        &[],
        0,
    );
    let claim_val: Value = serde_json::from_str(claim_res.stdout.trim()).unwrap();
    assert_eq!(
        object_keys(&claim_val),
        vec!["item", "holder", "token", "epoch", "expires_at"]
    );
    let Value::Str(tok1) = object_field(&claim_val, "token").clone() else {
        panic!("expected token string");
    };
    let Value::Str(claim1_exp) = object_field(&claim_val, "expires_at").clone() else {
        panic!("expected expires_at string");
    };
    let hash1 = dibs::token_hash(&tok1);

    // 4. claim-next (-j before subcommand)
    let next_res = run_cli_stdin_env(
        &[
            "-j",
            "--registry",
            path_str,
            "claim-next",
            "--holder",
            "bob",
            "--ttl",
            "600",
        ],
        None,
        &[],
        0,
    );
    let next_val: Value = serde_json::from_str(next_res.stdout.trim()).unwrap();
    assert_eq!(
        object_keys(&next_val),
        vec!["item", "holder", "token", "epoch", "expires_at"]
    );
    assert_eq!(
        object_field(&next_val, "item"),
        &Value::Str("task-two".to_string())
    );
    let Value::Str(tok2) = object_field(&next_val, "token").clone() else {
        panic!("expected token string");
    };
    let Value::Str(claim2_exp) = object_field(&next_val, "expires_at").clone() else {
        panic!("expected expires_at string");
    };
    let hash2 = dibs::token_hash(&tok2);

    // 5. renew (--json)
    let renew_res = run_cli_stdin_env(
        &[
            "--registry",
            path_str,
            "renew",
            "task-one",
            "--holder",
            "alice",
            "--token",
            &tok1,
            "--epoch",
            "1",
            "--ttl",
            "120",
            "--json",
        ],
        None,
        &[],
        0,
    );
    let renew_val: Value = serde_json::from_str(renew_res.stdout.trim()).unwrap();
    assert_eq!(
        object_keys(&renew_val),
        vec!["item", "holder", "epoch", "expires_at"]
    );
    assert_eq!(
        object_field(&renew_val, "item"),
        &Value::Str("task-one".to_string())
    );
    assert_eq!(
        object_field(&renew_val, "holder"),
        &Value::Str("alice".to_string())
    );
    assert_eq!(object_field(&renew_val, "epoch"), &Value::Int(1));
    let expected_renew_exp =
        dibs::format_instant(&dibs::parse_instant(&claim1_exp).unwrap().plus_seconds(120));
    assert_eq!(
        object_field(&renew_val, "expires_at"),
        &Value::Str(expected_renew_exp.clone())
    );

    // 6. board (--json) & 7. show (--json) while leases stand
    let detail_keys = vec![
        "id",
        "note",
        "status",
        "effective_status",
        "priority",
        "tags",
        "depends_on",
        "holder",
        "expires_at",
        "epoch",
        "takeover_count",
        "max_takeovers",
    ];
    let board_res = run_cli_stdin_env(&["--registry", path_str, "board", "--json"], None, &[], 0);
    let Value::Array(board_items) = serde_json::from_str::<Value>(board_res.stdout.trim()).unwrap()
    else {
        panic!("expected array");
    };
    assert_eq!(board_items.len(), 3);
    for elem in &board_items {
        assert_eq!(object_keys(elem), detail_keys);
        assert_eq!(object_field(elem, "priority"), &Value::Int(2));
        assert_eq!(object_field(elem, "tags"), &Value::Array(vec![]));
        assert_eq!(object_field(elem, "depends_on"), &Value::Array(vec![]));
        assert_eq!(object_field(elem, "takeover_count"), &Value::Int(0));
        assert_eq!(object_field(elem, "max_takeovers"), &Value::Null);
    }
    assert_eq!(
        object_field(&board_items[0], "id"),
        &Value::Str("task-one".to_string())
    );
    assert_eq!(
        object_field(&board_items[0], "note"),
        &Value::Str("alpha".to_string())
    );
    assert_eq!(
        object_field(&board_items[0], "status"),
        &Value::Str("claimed".to_string())
    );
    assert_eq!(
        object_field(&board_items[0], "effective_status"),
        &Value::Str("claimed".to_string())
    );
    assert_eq!(
        object_field(&board_items[0], "holder"),
        &Value::Str("alice".to_string())
    );
    assert_eq!(
        object_field(&board_items[0], "expires_at"),
        &Value::Str(expected_renew_exp)
    );
    assert_eq!(object_field(&board_items[0], "epoch"), &Value::Int(1));

    assert_eq!(
        object_field(&board_items[1], "id"),
        &Value::Str("task-two".to_string())
    );
    assert_eq!(
        object_field(&board_items[1], "holder"),
        &Value::Str("bob".to_string())
    );
    assert_eq!(
        object_field(&board_items[1], "expires_at"),
        &Value::Str(claim2_exp)
    );
    assert_eq!(
        object_field(&board_items[2], "id"),
        &Value::Str("task-three".to_string())
    );
    assert_eq!(
        object_field(&board_items[2], "status"),
        &Value::Str("open".to_string())
    );
    assert_eq!(
        object_field(&board_items[2], "effective_status"),
        &Value::Str("open".to_string())
    );
    assert_eq!(object_field(&board_items[2], "holder"), &Value::Null);
    assert_eq!(object_field(&board_items[2], "expires_at"), &Value::Null);
    assert_eq!(object_field(&board_items[2], "epoch"), &Value::Int(0));

    let show_res = run_cli_stdin_env(
        &["--registry", path_str, "show", "task-one", "--json"],
        None,
        &[],
        0,
    );
    let show_val: Value = serde_json::from_str(show_res.stdout.trim()).unwrap();
    assert_eq!(object_keys(&show_val), detail_keys);
    assert_eq!(show_val, board_items[0]);

    // 8. release (--json)
    let release_res = run_cli_stdin_env(
        &[
            "--registry",
            path_str,
            "release",
            "task-two",
            "--holder",
            "bob",
            "--token",
            &tok2,
            "--epoch",
            "1",
            "--json",
        ],
        None,
        &[],
        0,
    );
    let release_val: Value = serde_json::from_str(release_res.stdout.trim()).unwrap();
    assert_eq!(object_keys(&release_val), vec!["item", "status", "epoch"]);
    assert_eq!(
        release_res.stdout,
        "{\"item\": \"task-two\", \"status\": \"open\", \"epoch\": 1}\n"
    );

    // 9. complete (--json)
    let complete_res = run_cli_stdin_env(
        &[
            "--registry",
            path_str,
            "complete",
            "task-one",
            "--holder",
            "alice",
            "--token",
            &tok1,
            "--epoch",
            "1",
            "--json",
        ],
        None,
        &[],
        0,
    );
    let complete_val: Value = serde_json::from_str(complete_res.stdout.trim()).unwrap();
    assert_eq!(object_keys(&complete_val), vec!["item", "status", "epoch"]);
    assert_eq!(
        complete_res.stdout,
        "{\"item\": \"task-one\", \"status\": \"completed\", \"epoch\": 1}\n"
    );

    // 10. history (--json)
    let history_res =
        run_cli_stdin_env(&["--registry", path_str, "history", "--json"], None, &[], 0);
    let Value::Array(events) = serde_json::from_str::<Value>(history_res.stdout.trim()).unwrap()
    else {
        panic!("expected history array");
    };
    assert_eq!(events.len(), 8);
    let event_keys = vec![
        "seq",
        "at",
        "kind",
        "item",
        "holder",
        "epoch",
        "expires_at",
        "note",
        "previous_holder",
    ];
    let expected_history: [(i128, &str, &str, Option<&str>, i128); 8] = [
        (1, "offered", "task-one", None, 0),
        (2, "offered", "task-two", None, 0),
        (3, "offered", "task-three", None, 0),
        (4, "claimed", "task-one", Some("alice"), 1),
        (5, "claimed", "task-two", Some("bob"), 1),
        (6, "renewed", "task-one", Some("alice"), 1),
        (7, "released", "task-two", Some("bob"), 1),
        (8, "completed", "task-one", Some("alice"), 1),
    ];
    for (ev, (seq, kind, item_name, holder, epoch)) in events.iter().zip(expected_history) {
        assert_eq!(object_keys(ev), event_keys);
        assert_eq!(object_field(ev, "seq"), &Value::Int(seq));
        assert_eq!(object_field(ev, "kind"), &Value::Str(kind.to_string()));
        assert_eq!(object_field(ev, "item"), &Value::Str(item_name.to_string()));
        let exp_holder = holder
            .map(|h| Value::Str(h.to_string()))
            .unwrap_or(Value::Null);
        assert_eq!(object_field(ev, "holder"), &exp_holder);
        assert_eq!(object_field(ev, "epoch"), &Value::Int(epoch));
        assert_eq!(object_field(ev, "expires_at"), &Value::Null);
        assert_eq!(object_field(ev, "note"), &Value::Null);
        assert_eq!(object_field(ev, "previous_holder"), &Value::Null);
    }

    // Strict single-line JSON check and zero secret leakage check across all 10 subcommands.
    for out_text in [
        &offer_res.stdout,
        &batch_res.stdout,
        &claim_res.stdout,
        &next_res.stdout,
        &renew_res.stdout,
        &board_res.stdout,
        &show_res.stdout,
        &release_res.stdout,
        &complete_res.stdout,
        &history_res.stdout,
    ] {
        assert!(out_text.ends_with('\n'), "{out_text}");
        assert!(
            !out_text[..out_text.len() - 1].contains(['\n', '\r']),
            "multiline JSON output: {out_text:?}"
        );
        assert!(!out_text.contains("token_hash"), "{out_text}");
        assert!(!out_text.contains(&hash1), "{out_text}");
        assert!(!out_text.contains(&hash2), "{out_text}");
    }
    for non_claim_out in [
        &offer_res.stdout,
        &batch_res.stdout,
        &renew_res.stdout,
        &board_res.stdout,
        &show_res.stdout,
        &release_res.stdout,
        &complete_res.stdout,
        &history_res.stdout,
    ] {
        assert!(!non_claim_out.contains(&tok1), "{non_claim_out}");
        assert!(!non_claim_out.contains(&tok2), "{non_claim_out}");
    }
}

#[test]
fn cli_safe_credentials_env_fallbacks_version_and_completions() {
    let dir = temp_dir("cli-safe-creds");
    let path = dir.join("registry.json");
    let path_str = path.to_str().unwrap();

    // Missing --registry when DIBS_REGISTRY is unset fails at exit 2 with parser error.
    let missing_reg = run_cli_stdin_env(&["board"], None, &[], 2);
    assert!(
        missing_reg
            .stderr
            .contains("the following arguments are required: --registry"),
        "{}",
        missing_reg.stderr
    );

    // Missing --token on renew, release, and complete when DIBS_TOKEN is unset
    // fails at exit 2 with parser error.
    for missing_tok_args in [
        vec![
            "--registry",
            path_str,
            "renew",
            "task-env",
            "--holder",
            "alice",
            "--epoch",
            "1",
            "--ttl",
            "60",
        ],
        vec![
            "--registry",
            path_str,
            "release",
            "task-env",
            "--holder",
            "alice",
            "--epoch",
            "1",
        ],
        vec![
            "--registry",
            path_str,
            "complete",
            "task-env",
            "--holder",
            "alice",
            "--epoch",
            "1",
        ],
    ] {
        let missing_tok = run_cli_stdin_env(&missing_tok_args, None, &[], 2);
        assert!(
            missing_tok
                .stderr
                .contains("the following arguments are required: --token"),
            "{}",
            missing_tok.stderr
        );
    }

    // DIBS_REGISTRY fallback and post-subcommand --registry override.
    run_cli_stdin_env(
        &["offer", "task-env", "--note", "from env"],
        None,
        &[("DIBS_REGISTRY", Some(path_str))],
        0,
    );
    let other_path = dir.join("other.json");
    let other_str = other_path.to_str().unwrap();
    run_cli_stdin_env(
        &["offer", "task-override", "--registry", other_str],
        None,
        &[("DIBS_REGISTRY", Some(path_str))],
        0,
    );
    let env_board = run_cli_stdin_env(&["board"], None, &[("DIBS_REGISTRY", Some(path_str))], 0);
    assert_eq!(env_board.stdout, "task-env open epoch=0\n");
    let post_board = run_cli_stdin_env(&["board", "--registry", other_str], None, &[], 0);
    assert_eq!(post_board.stdout, "task-override open epoch=0\n");

    // Empty DIBS_REGISTRY="" triggers DB-15 path validation at exit 2.
    let empty_env_reg = run_cli_stdin_env(&["board"], None, &[("DIBS_REGISTRY", Some(""))], 2);
    assert_eq!(
        empty_env_reg.stderr,
        "error: invalid registry path '', the path must name a file\n"
    );

    // Non-UTF-8 stdin bytes on offer-batch refuse at exit 2.
    let bad_batch_utf8 = run_cli_stdin_env(
        &["offer-batch"],
        Some(b"bad\xff\n"),
        &[("DIBS_REGISTRY", Some(path_str))],
        2,
    );
    assert!(
        bad_batch_utf8
            .stderr
            .starts_with("error: offer batch input is not valid UTF-8"),
        "{}",
        bad_batch_utf8.stderr
    );

    // Claim task-env, then exercise --token - round-trip and boundaries.
    let claim_out = run_cli_stdin_env(
        &["claim-next", "--holder", "alice", "--ttl", "600", "--json"],
        None,
        &[("DIBS_REGISTRY", Some(path_str))],
        0,
    );
    let claim_val: Value = serde_json::from_str(claim_out.stdout.trim()).unwrap();
    let Value::Str(tok) = object_field(&claim_val, "token").clone() else {
        panic!("expected token");
    };

    // Non-UTF-8 stdin bytes on --token - refuse at exit 2.
    let bad_tok_utf8 = run_cli_stdin_env(
        &[
            "renew", "task-env", "--holder", "alice", "--token", "-", "--epoch", "1", "--ttl", "60",
        ],
        Some(b"bad\xff\n"),
        &[("DIBS_REGISTRY", Some(path_str))],
        2,
    );
    assert_eq!(
        bad_tok_utf8.stderr,
        "error: stdin token is not valid UTF-8\n"
    );

    // --token - empty EOF and CRLF-only refuse at exit 2.
    for empty_in in [b"".as_slice(), b"\n".as_slice(), b"\r\n".as_slice()] {
        let res = run_cli_stdin_env(
            &[
                "renew", "task-env", "--holder", "alice", "--token", "-", "--epoch", "1", "--ttl",
                "60",
            ],
            Some(empty_in),
            &[("DIBS_REGISTRY", Some(path_str))],
            2,
        );
        assert_eq!(res.stderr, "error: stdin token is empty\n");
    }

    // --token - boundary: 256 bytes passes length guard (reaches possession check at exit 1),
    // whereas 257 bytes refuses at exit 2 with "stdin token exceeds 256 bytes".
    let at_256 = vec![b'x'; 256];
    let res_256 = run_cli_stdin_env(
        &[
            "renew", "task-env", "--holder", "alice", "--token", "-", "--epoch", "1", "--ttl", "60",
        ],
        Some(&at_256),
        &[("DIBS_REGISTRY", Some(path_str))],
        1,
    );
    assert_eq!(
        res_256.stderr,
        "error: the token presented for item 'task-env' does not match the lease\n"
    );

    let at_257 = vec![b'x'; 257];
    let res_257 = run_cli_stdin_env(
        &[
            "renew", "task-env", "--holder", "alice", "--token", "-", "--epoch", "1", "--ttl", "60",
        ],
        Some(&at_257),
        &[("DIBS_REGISTRY", Some(path_str))],
        2,
    );
    assert_eq!(res_257.stderr, "error: stdin token exceeds 256 bytes\n");

    // --token - valid token with trailing CRLF succeeds on renew AND on release!
    let stdin_tok = format!("{tok}\r\n");
    run_cli_stdin_env(
        &[
            "renew", "task-env", "--holder", "alice", "--token", "-", "--epoch", "1", "--ttl", "60",
        ],
        Some(stdin_tok.as_bytes()),
        &[("DIBS_REGISTRY", Some(path_str))],
        0,
    );
    let rel_stdin = run_cli_stdin_env(
        &[
            "release", "task-env", "--holder", "alice", "--token", "-", "--epoch", "1",
        ],
        Some(stdin_tok.as_bytes()),
        &[("DIBS_REGISTRY", Some(path_str))],
        0,
    );
    assert_eq!(rel_stdin.stdout, "released task-env\n");

    // Re-claim at epoch 2, release via DIBS_TOKEN, then re-claim at epoch 3 and complete via DIBS_TOKEN.
    let claim2_out = run_cli_stdin_env(
        &["claim-next", "--holder", "alice", "--ttl", "600", "--json"],
        None,
        &[("DIBS_REGISTRY", Some(path_str))],
        0,
    );
    let claim2_val: Value = serde_json::from_str(claim2_out.stdout.trim()).unwrap();
    let Value::Str(tok2) = object_field(&claim2_val, "token").clone() else {
        panic!("expected token");
    };
    let rel_env = run_cli_stdin_env(
        &["release", "task-env", "--holder", "alice", "--epoch", "2"],
        None,
        &[
            ("DIBS_REGISTRY", Some(path_str)),
            ("DIBS_TOKEN", Some(&tok2)),
        ],
        0,
    );
    assert_eq!(rel_env.stdout, "released task-env\n");

    let claim3_out = run_cli_stdin_env(
        &["claim-next", "--holder", "alice", "--ttl", "600", "--json"],
        None,
        &[("DIBS_REGISTRY", Some(path_str))],
        0,
    );
    let claim3_val: Value = serde_json::from_str(claim3_out.stdout.trim()).unwrap();
    let Value::Str(tok3) = object_field(&claim3_val, "token").clone() else {
        panic!("expected token");
    };

    // Empty DIBS_TOKEN="" refuses at exit 2, while valid DIBS_TOKEN completes cleanly.
    let empty_env_tok = run_cli_stdin_env(
        &["complete", "task-env", "--holder", "alice", "--epoch", "3"],
        None,
        &[("DIBS_REGISTRY", Some(path_str)), ("DIBS_TOKEN", Some(""))],
        2,
    );
    assert_eq!(empty_env_tok.stderr, "error: DIBS_TOKEN is empty\n");

    let done = run_cli_stdin_env(
        &["complete", "task-env", "--holder", "alice", "--epoch", "3"],
        None,
        &[
            ("DIBS_REGISTRY", Some(path_str)),
            ("DIBS_TOKEN", Some(&tok3)),
        ],
        0,
    );
    assert_eq!(done.stdout, "completed task-env\n");

    // Standalone -V and --version succeed without --registry; clusters and --json combos exit 2.
    let expected_ver = format!("dibs {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(
        run_cli_stdin_env(&["-V"], None, &[], 0).stdout,
        expected_ver
    );
    assert_eq!(
        run_cli_stdin_env(&["--version"], None, &[], 0).stdout,
        expected_ver
    );
    for bad_ver in [
        vec!["-jV"],
        vec!["-Vj"],
        vec!["-hj"],
        vec!["--json", "--version"],
        vec!["--version", "--json"],
    ] {
        run_cli_stdin_env(&bad_ver, None, &[], 2);
    }

    // Completions for bash, zsh, and fish succeed without --registry and cover
    // all 14 subcommands and all 16 option flags (DB-S3 mechanical update for
    // DB-20, DB-22, DB-23, DB-24).
    for shell in ["bash", "zsh", "fish"] {
        let comp = run_cli_stdin_env(&["completions", shell], None, &[], 0);
        let bash_sub_words: Vec<&str> = if shell == "bash" {
            let line = comp
                .stdout
                .lines()
                .find(|l| l.trim_start().starts_with("local subcommands=\""))
                .expect("bash subcommands line");
            line.split('"').nth(1).unwrap().split_whitespace().collect()
        } else {
            Vec::new()
        };
        for sub in [
            "offer",
            "offer-batch",
            "claim",
            "claim-next",
            "renew",
            "release",
            "complete",
            "reset",
            "board",
            "ready",
            "show",
            "history",
            "compact",
            "completions",
        ] {
            match shell {
                "bash" => {
                    assert!(
                        bash_sub_words.contains(&sub),
                        "bash subcommands variable missing word {sub}"
                    );
                }
                "zsh" => {
                    assert!(
                        comp.stdout.contains(&format!("'{sub}:")),
                        "zsh completions missing subcommand anchor '{sub}:"
                    );
                }
                "fish" => {
                    assert!(
                        comp.stdout.contains(&format!("-a {sub} -d")),
                        "fish completions missing subcommand anchor -a {sub} -d"
                    );
                }
                _ => unreachable!(),
            }
        }
        for flag in [
            "--help",
            "--version",
            "--json",
            "--registry",
            "--note",
            "--priority",
            "--tag",
            "--depends-on",
            "--max-takeovers",
            "--file",
            "--holder",
            "--ttl",
            "--token",
            "--epoch",
            "--through-seq",
            "--archive",
        ] {
            assert!(
                comp.stdout.contains(flag),
                "{shell} completions missing flag {flag}"
            );
        }
        if shell == "fish" {
            for opt in [
                "-l help",
                "-l version",
                "-l json",
                "-l registry",
                "-l note",
                "-l priority",
                "-l tag",
                "-l depends-on",
                "-l max-takeovers",
                "-l file",
                "-l holder",
                "-l ttl",
                "-l token",
                "-l epoch",
                "-l through-seq",
                "-l archive",
            ] {
                assert!(
                    comp.stdout.contains(opt),
                    "fish completions missing option rule {opt}"
                );
            }
        }
    }
    #[cfg(unix)]
    {
        for shell in ["bash", "zsh"] {
            if let Ok(status) = Command::new(shell).arg("-c").arg("exit 0").status() {
                if status.success() {
                    let comp = run_cli_stdin_env(&["completions", shell], None, &[], 0);
                    let mut check = Command::new(shell)
                        .arg("-n")
                        .stdin(std::process::Stdio::piped())
                        .spawn()
                        .unwrap();
                    use std::io::Write;
                    check
                        .stdin
                        .take()
                        .unwrap()
                        .write_all(comp.stdout.as_bytes())
                        .unwrap();
                    assert!(
                        check.wait().unwrap().success(),
                        "{shell} -n failed on generated completion script"
                    );
                }
            }
        }
    }
    for bad_comp in [
        vec!["completions"],
        vec!["completions", "powershell"],
        vec!["completions", "zsh", "--json"],
        vec!["--json", "completions", "zsh"],
        vec!["completions", "zsh", "-j"],
    ] {
        run_cli_stdin_env(&bad_comp, None, &[], 2);
    }
}

#[cfg(unix)]
#[test]
fn cli_post_subcommand_registry_preserves_non_unicode_os_string_bytes() {
    use std::os::unix::ffi::OsStringExt;

    let dir = temp_dir("cli-post-sub-non-unicode");
    let odd_dir = dir.join(std::ffi::OsString::from_vec(vec![0xff]));
    let odd_path = odd_dir.join("registry.json");
    let mut equals = std::ffi::OsString::from("--registry=");
    equals.push(odd_path.as_os_str());

    if std::fs::create_dir_all(&odd_dir).is_ok() {
        for post_args in [
            vec![
                std::ffi::OsString::from("offer"),
                std::ffi::OsString::from("task-one"),
                std::ffi::OsString::from("--registry"),
                odd_path.clone().into(),
            ],
            vec![std::ffi::OsString::from("board"), equals.clone()],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_dibs"))
                .env_remove("DIBS_REGISTRY")
                .args(&post_args)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(0), "{stderr}");
            assert!(!stderr.contains("not valid Unicode text"), "{stderr}");
        }
    } else {
        for post_args in [
            vec![
                std::ffi::OsString::from("board"),
                std::ffi::OsString::from("--registry"),
                odd_path.clone().into(),
            ],
            vec![std::ffi::OsString::from("board"), equals],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_dibs"))
                .env_remove("DIBS_REGISTRY")
                .args(&post_args)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(0), "{stderr}");
            assert_eq!(output.stdout, b"");
            assert!(!stderr.contains("not valid Unicode text"), "{stderr}");
            assert!(!stderr.contains("panicked"), "{stderr}");
        }
    }

    // Non-Unicode bytes on a non-path subcommand option value (--note) still
    // refuse as a Unicode parser error at exit 2.
    let valid_path = dir.join("valid.json");
    let bad_note = std::ffi::OsString::from_vec(vec![0xff, 0xfe]);
    let bad_out = Command::new(env!("CARGO_BIN_EXE_dibs"))
        .env_remove("DIBS_REGISTRY")
        .arg("--registry")
        .arg(&valid_path)
        .arg("offer")
        .arg("task-one")
        .arg("--note")
        .arg(&bad_note)
        .output()
        .unwrap();
    let bad_stderr = String::from_utf8_lossy(&bad_out.stderr);
    assert_eq!(bad_out.status.code(), Some(2), "{bad_stderr}");
    assert!(
        bad_stderr.contains("not valid Unicode text"),
        "{bad_stderr}"
    );
}
