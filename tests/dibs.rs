//! Tests for the dibs claim lease protocol, slice 1, translated
//! pin for pin from the Round 1 Python suite.
//!
//! Deterministic time comes only from injected instants. No test
//! sleeps to make a lease expire, and the only wall clock use is
//! the command line smoke path where no expiry boundary is
//! crossed. Fixtures are synthetic and written for this repo.
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
    check_possession, validate_name, validate_note, validate_ttl, Error, Instant, Item, Registry,
    Value,
};

fn t0() -> Instant {
    Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(0)).unwrap()
}

fn at(seconds: i64) -> Instant {
    t0().plus_seconds(seconds)
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dibs-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_registry(dir: &Path) -> Registry {
    Registry::new(dir.join("registry.json"))
}

fn expect_error<T>(result: dibs::Result<T>) -> Error {
    match result {
        Ok(_) => panic!("the operation succeeded where a refusal was expected"),
        Err(error) => error,
    }
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
            let dir = temp_dir(stringify!($name));
            let registry = make_registry(&dir);
            registry
                .offer("task-one", "a synthetic task", &t0())
                .unwrap();
            // A time to live of this shape reaches the validator
            // the claim and the renew both call first, and it
            // refuses there with the typed error, before any
            // state is read.
            assert_invalid_ttl(validate_ttl(&$value));
            let granted = registry.claim("task-one", "agent-a", 60, &t0()).unwrap();
            let _ = granted;
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
    let base = temp_dir("damage-format");
    let (directory, text) = mutated(&base, "case", |doc| {
        doc["format"] = serde_json::json!("dibs-registry/2");
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
    let mut out = String::from("{\"format\": \"dibs-registry/1\", \"items\": [");
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"id\": \"{id}\", \"note\": \"\", \"status\": \"open\", \
             \"holder\": null, \"token_hash\": null, \"epoch\": 0, \"expires_at\": null}}"
        ));
    }
    out.push_str("], \"history\": [");
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"seq\": {}, \"kind\": \"offered\", \"item\": \"{id}\", \
             \"holder\": null, \"at\": \"2031-05-04T12:00:00+00:00\", \"epoch\": 0, \
             \"previous_holder\": null}}",
            index + 1
        ));
    }
    out.push_str("]}");
    out
}

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

struct CliOutput {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_cli(args: &[&str], expect: i32) -> CliOutput {
    let output = Command::new(env!("CARGO_BIN_EXE_dibs"))
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
    assert!(lines[0].starts_with("token "));
    let token = lines[0].split_once(' ').unwrap().1;
    assert_eq!(lines[1], "epoch 1");
    let board = run_cli(&["--registry", path, "board"], 0);
    assert!(board.stdout.contains("task-one claimed holder=agent-a"));
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
fn cli_surface_carries_no_time_override_flag() {
    let help = run_cli(&["--help"], 0);
    assert!(!help.stdout.contains("--now"));
    assert!(!help.stdout.contains("--at"));
    let dir = temp_dir("cli-no-override");
    let path = dir.join("registry.json");
    let path = path.to_str().unwrap();
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
