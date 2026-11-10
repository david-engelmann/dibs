//! Equivalence proofs beyond translation, the pins the port
//! amendment names beside the translated suite.
//!
//! Eight registry files written by the Python implementation on
//! main stand frozen in the fixtures folder. These pins check
//! their hashes against the manifest inventory, read their board
//! and history truths under Rust, and continue their standing
//! claims with the tokens the manifest records. One pin replays
//! an offers only production under Rust and compares the bytes
//! whole, and one compares a claim production field by field
//! with the hash checked against the drawn token. The last pins
//! hold the arithmetic edges, stored counts at the magnitudes
//! the sibling ports overflowed.

use std::path::PathBuf;

use dibs::{Error, Instant, Registry};
use sha2::{Digest, Sha256};

fn t0() -> Instant {
    Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(0)).unwrap()
}

fn at(seconds: i64) -> Instant {
    t0().plus_seconds(seconds)
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dibs-fixture-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Copy one frozen fixture into a scratch directory and open it
/// there, so the frozen bytes in the tree are never the file a
/// test writes to or locks beside.
fn staged_fixture(tag: &str, name: &str) -> (PathBuf, Registry) {
    let dir = temp_dir(tag);
    let path = dir.join(name);
    std::fs::copy(fixture_path(name), &path).unwrap();
    let registry = Registry::new(path);
    (dir, registry)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn expect_error<T>(result: dibs::Result<T>) -> Error {
    match result {
        Ok(_) => panic!("the operation succeeded where a refusal was expected"),
        Err(error) => error,
    }
}

/// Stage one hand written document in a scratch directory and
/// open it there.
fn staged_document(tag: &str, text: &str) -> (PathBuf, Registry) {
    let dir = temp_dir(tag);
    let path = dir.join("registry.json");
    std::fs::write(&path, text).unwrap();
    let registry = Registry::new(path);
    (dir, registry)
}

/// A document holding one claimed item under a known token,
/// with the expiry spelled as the case under test names it.
fn claimed_document(expires_at: &str) -> String {
    format!(
        "{{\"format\": \"dibs-registry/1\", \"items\": [{{\"id\": \"task-one\", \
         \"note\": \"\", \"status\": \"claimed\", \"holder\": \"agent-a\", \
         \"token_hash\": \"{}\", \"epoch\": 1, \"expires_at\": \"{}\"}}], \
         \"history\": []}}",
        dibs::token_hash("standing-token"),
        expires_at
    )
}

/// The DB-14 statement's field lists and vocabularies, as
/// lists of names in construction order.
type ContractTable = (
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
);

/// Read the DB-14 statement out of CONTRACT.md and return its
/// three field lists in construction order beside the two
/// vocabularies, as lists of names.
fn contract_table() -> ContractTable {
    let text =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("CONTRACT.md"))
            .unwrap();
    let start = text.find("Obligation DB-14 covers").unwrap();
    let after = &text[start..];
    let end = after.find("The cost of working a registry").unwrap();
    let db14 = after[..end].replace('\n', " ");
    let field_list = |anchor: &str| -> Vec<String> {
        let at = db14.find(anchor).unwrap();
        let rest = &db14[at + anchor.len()..];
        let stop = rest.find(", in that construction order").unwrap();
        let phrase = rest[..stop].strip_prefix("exactly the fields ").unwrap();
        phrase
            .split(" and ")
            .flat_map(|part| part.split(", "))
            .map(|name| name.to_string())
            .collect()
    };
    let vocabulary = |anchor: &str| -> Vec<String> {
        let at = db14.find(anchor).unwrap();
        let rest = &db14[at + anchor.len()..];
        let stop = rest.find('.').unwrap();
        rest[..stop]
            .split(" and ")
            .map(|name| name.to_string())
            .collect()
    };
    (
        field_list("document is one JSON object carrying "),
        field_list("Each item object carries "),
        field_list("Each event object carries "),
        vocabulary("The status field carries one of "),
        vocabulary("The kind field carries one of "),
    )
}

/// Check a list of names against the stated set, as a set.
fn assert_key_set(got: Vec<&str>, stated: &[String], where_: &str) {
    let stated_set: std::collections::BTreeSet<&str> = stated.iter().map(String::as_str).collect();
    let got_set: std::collections::BTreeSet<&str> = got.iter().copied().collect();
    assert_eq!(
        got_set.len(),
        got.len(),
        "{where_} carries a duplicated key"
    );
    assert_eq!(got_set, stated_set, "{where_}");
}

#[test]
fn contract_field_table_matches_the_frozen_fixtures() {
    let (document_keys, item_keys, event_keys, statuses, kinds) = contract_table();
    let names = [
        "empty.json",
        "offers.json",
        "claimed.json",
        "renewed.json",
        "released.json",
        "completed.json",
        "taken-over.json",
        "mixed.json",
    ];
    for name in names {
        let raw = std::fs::read(fixture_path(name)).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let document = value.as_object().unwrap();
        assert_key_set(
            document.keys().map(String::as_str).collect(),
            &document_keys,
            name,
        );
        for item in document["items"].as_array().unwrap() {
            assert_key_set(
                item.as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect(),
                &item_keys,
                name,
            );
            let status = item["status"].as_str().unwrap();
            assert!(
                statuses.iter().any(|stated| stated == status),
                "{name} carries status {status} outside the table"
            );
        }
        for event in document["history"].as_array().unwrap() {
            assert_key_set(
                event
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect(),
                &event_keys,
                name,
            );
            let kind = event["kind"].as_str().unwrap();
            assert!(
                kinds.iter().any(|stated| stated == kind),
                "{name} carries kind {kind} outside the table"
            );
        }
    }
}

/// One registry document built by hand from the DB-14 table
/// alone. One item stands open with no events, one stands
/// claimed with its lease fields standing, and the two events
/// the claimed item's life produces stand recorded.
fn stranger_document(token_spelling: &str) -> String {
    format!(
        "{{\"format\": \"dibs-registry/1\", \"items\": [\
         {{\"id\": \"task-alpha\", \"note\": \"alpha payload\", \"status\": \"open\", \
         \"holder\": null, \"token_hash\": null, \"epoch\": 0, \"expires_at\": null}}, \
         {{\"id\": \"task-beta\", \"note\": \"beta payload\", \"status\": \"claimed\", \
         \"holder\": \"agent-a\", \"{token_spelling}\": \"{}\", \"epoch\": 1, \
         \"expires_at\": \"2031-05-04T12:10:00+00:00\"}}], \
         \"history\": [\
         {{\"seq\": 1, \"kind\": \"offered\", \"item\": \"task-beta\", \
         \"holder\": null, \"at\": \"2031-05-04T12:00:00+00:00\", \"epoch\": 0, \
         \"previous_holder\": null}}, \
         {{\"seq\": 2, \"kind\": \"claimed\", \"item\": \"task-beta\", \
         \"holder\": \"agent-a\", \"at\": \"2031-05-04T12:00:10+00:00\", \"epoch\": 1, \
         \"previous_holder\": null}}]}}",
        dibs::token_hash("stranger-token")
    )
}

#[test]
fn a_document_built_from_the_contract_table_alone_reads_clean() {
    let (_, registry) = staged_document("stranger-clean", &stranger_document("token_hash"));
    let rows = registry.board(&t0()).unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| {
                (
                    row.id.clone(),
                    row.status.clone(),
                    row.holder.clone(),
                    row.epoch,
                    row.expires_at.clone(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            ("task-alpha".to_string(), "open".to_string(), None, 0, None,),
            (
                "task-beta".to_string(),
                "claimed".to_string(),
                Some("agent-a".to_string()),
                1,
                Some("2031-05-04T12:10:00+00:00".to_string()),
            ),
        ]
    );
    let events = registry.history(None).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[0].kind, "offered");
    assert_eq!(events[0].item, "task-beta");
    assert_eq!(events[0].holder, None);
    assert_eq!(events[0].at, "2031-05-04T12:00:00+00:00");
    assert_eq!(events[0].epoch, 0);
    assert_eq!(events[0].previous_holder, None);
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[1].kind, "claimed");
    assert_eq!(events[1].item, "task-beta");
    assert_eq!(events[1].holder.as_deref(), Some("agent-a"));
    assert_eq!(events[1].at, "2031-05-04T12:00:10+00:00");
    assert_eq!(events[1].epoch, 1);
    assert_eq!(events[1].previous_holder, None);
    let filtered = registry.history(Some("task-beta")).unwrap();
    assert_eq!(filtered.len(), 2);
    assert!(registry.history(Some("task-alpha")).unwrap().is_empty());
}

#[test]
fn a_document_with_one_field_respelled_refuses_as_corruption() {
    let text = stranger_document("token-hash");
    let (dir, registry) = staged_document("stranger-respelled", &text);
    let path = dir.join("registry.json");
    let before = std::fs::read(&path).unwrap();
    for error in [
        expect_error(registry.board(&t0())),
        expect_error(registry.history(None)),
        expect_error(registry.history(Some("task-alpha"))),
        expect_error(registry.offer("task-gamma", "gamma payload", &t0())),
        expect_error(registry.claim("task-alpha", "agent-b", 600, &t0())),
    ] {
        assert!(
            matches!(error, Error::Corrupt(_)),
            "the respelled document refused as {error:?}, not corruption"
        );
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn fixture_hashes_match_the_manifest_inventory() {
    let inventory: [(&str, usize, &str); 8] = [
        (
            "empty.json",
            66,
            "2cf0628996bdfcc24bdf5cff1133712dbde1798d5087893b8fa22a9574378b97",
        ),
        (
            "offers.json",
            825,
            "9bd88441ba5b04b7e2c039aa7bbf9530bab3c78d728509f2a52523f977dac025",
        ),
        (
            "claimed.json",
            739,
            "4af3d1169347fe15ab3570b12c1250c466af76cca75842ff955dc01eeebd31e9",
        ),
        (
            "renewed.json",
            937,
            "3db3e42b1f1394f83ee6d0bcd82dee72adf4fc883d619433963f389e571328f7",
        ),
        (
            "released.json",
            845,
            "c72e35709b9ace2a79aa28ba1e96851c8eb57ec91cf0dabe6b842c95d246a01b",
        ),
        (
            "completed.json",
            851,
            "5d0b8dcee187a888c539d6566b5b51b26f5688a403327aa81c3341b3c9ad93fc",
        ),
        (
            "taken-over.json",
            945,
            "a94e08110bfe5a0580a97cbfd792ac9e2eeb678f65a27936366add7f15de51b1",
        ),
        (
            "mixed.json",
            2563,
            "90acad5aaee8df5aa46b554a2f874b645eed9a46703af39c2b02848d3f470c6f",
        ),
    ];
    for (name, bytes, hash) in inventory {
        let raw = std::fs::read(fixture_path(name)).unwrap();
        assert_eq!(raw.len(), bytes, "{name}");
        assert_eq!(sha256_hex(&raw), hash, "{name}");
    }
}

/// One fixture board, as rows of id, status, holder, epoch,
/// and expiry.
type BoardShape = Vec<(String, String, Option<String>, i128, Option<String>)>;

/// One fixture history, as rows of seq, kind, item, holder,
/// epoch, and previous holder.
type HistoryShape = Vec<(i128, String, String, Option<String>, i128, Option<String>)>;

fn board_shape(registry: &Registry, now: &Instant) -> BoardShape {
    registry
        .board(now)
        .unwrap()
        .iter()
        .map(|row| {
            (
                row.id.clone(),
                row.status.clone(),
                row.holder.clone(),
                row.epoch,
                row.expires_at.clone(),
            )
        })
        .collect()
}

fn history_shape(registry: &Registry) -> HistoryShape {
    registry
        .history(None)
        .unwrap()
        .iter()
        .map(|event| {
            (
                event.seq,
                event.kind.clone(),
                event.item.clone(),
                event.holder.clone(),
                event.epoch,
                event.previous_holder.clone(),
            )
        })
        .collect()
}

#[test]
fn fixture_truths_read_back_under_rust() {
    let (_, empty) = staged_fixture("truths-empty", "empty.json");
    assert_eq!(board_shape(&empty, &at(20)), vec![]);
    assert_eq!(history_shape(&empty), vec![]);

    let (_, offers) = staged_fixture("truths-offers", "offers.json");
    assert_eq!(
        board_shape(&offers, &at(20)),
        vec![
            ("task-alpha".to_string(), "open".to_string(), None, 0, None),
            ("task-beta".to_string(), "open".to_string(), None, 0, None),
        ]
    );
    assert_eq!(
        history_shape(&offers),
        vec![
            (
                1,
                "offered".to_string(),
                "task-alpha".to_string(),
                None,
                0,
                None
            ),
            (
                2,
                "offered".to_string(),
                "task-beta".to_string(),
                None,
                0,
                None
            ),
        ]
    );

    let (_, claimed) = staged_fixture("truths-claimed", "claimed.json");
    assert_eq!(
        board_shape(&claimed, &at(20)),
        vec![(
            "task-alpha".to_string(),
            "claimed".to_string(),
            Some("agent-a".to_string()),
            1,
            Some("2031-05-04T12:10:10+00:00".to_string()),
        )]
    );

    let (_, renewed) = staged_fixture("truths-renewed", "renewed.json");
    assert_eq!(
        board_shape(&renewed, &at(20)),
        vec![(
            "task-alpha".to_string(),
            "claimed".to_string(),
            Some("agent-a".to_string()),
            1,
            Some("2031-05-04T12:15:00+00:00".to_string()),
        )]
    );
    let kinds: Vec<String> = history_shape(&renewed)
        .iter()
        .map(|event| event.1.clone())
        .collect();
    assert_eq!(kinds, vec!["offered", "claimed", "renewed"]);

    let (_, released) = staged_fixture("truths-released", "released.json");
    assert_eq!(
        board_shape(&released, &at(20)),
        vec![("task-alpha".to_string(), "open".to_string(), None, 1, None)]
    );

    let (_, completed) = staged_fixture("truths-completed", "completed.json");
    assert_eq!(board_shape(&completed, &at(20)), vec![]);
    assert_eq!(history_shape(&completed).len(), 3);

    let (_, taken) = staged_fixture("truths-taken", "taken-over.json");
    assert_eq!(
        board_shape(&taken, &at(20)),
        vec![(
            "task-alpha".to_string(),
            "claimed".to_string(),
            Some("agent-b".to_string()),
            2,
            Some("2031-05-04T12:12:00+00:00".to_string()),
        )]
    );
    let last = history_shape(&taken).pop().unwrap();
    assert_eq!(last.1, "taken_over");
    assert_eq!(last.5.as_deref(), Some("agent-a"));

    let (_, mixed) = staged_fixture("truths-mixed", "mixed.json");
    assert_eq!(
        board_shape(&mixed, &at(20)),
        vec![
            ("task-alpha".to_string(), "open".to_string(), None, 0, None),
            (
                "task-beta".to_string(),
                "claimed".to_string(),
                Some("agent-a".to_string()),
                1,
                Some("2031-05-04T12:10:04+00:00".to_string()),
            ),
            (
                "task-gamma".to_string(),
                "claimed".to_string(),
                Some("agent-c".to_string()),
                1,
                Some("2031-05-04T12:00:35+00:00".to_string()),
            ),
        ]
    );
    assert_eq!(history_shape(&mixed).len(), 8);
}

#[test]
fn fixture_registries_pass_the_cross_validation() {
    // The control for the Round 2 cross validation. Every
    // registry the Python implementation wrote and froze must
    // still read clean now that the reader joins the history
    // to the standing state.
    let names = [
        "empty.json",
        "offers.json",
        "claimed.json",
        "renewed.json",
        "released.json",
        "completed.json",
        "taken-over.json",
        "mixed.json",
    ];
    for name in names {
        let (_, registry) = staged_fixture(&format!("cross-{name}"), name);
        registry.board(&t0()).unwrap();
        registry.history(None).unwrap();
    }
}

#[test]
fn fixture_claim_renews_and_completes_under_rust() {
    let (_, registry) = staged_fixture("continue-claimed", "claimed.json");
    let token = "J-55nqLzVcGYFXUo0PG0Y-jSixW_9ub2u_FI3GTSNhw";
    let expires = registry
        .renew("task-alpha", "agent-a", token, 1, 60, &at(30))
        .unwrap();
    assert_eq!(dibs::format_instant(&expires), "2031-05-04T12:11:10+00:00");
    registry
        .complete("task-alpha", "agent-a", token, 1, &at(40))
        .unwrap();
    assert_eq!(board_shape(&registry, &at(50)), vec![]);
    let kinds: Vec<String> = history_shape(&registry)
        .iter()
        .map(|event| event.1.clone())
        .collect();
    assert_eq!(kinds, vec!["offered", "claimed", "renewed", "completed"]);
}

#[test]
fn fixture_renewed_claim_completes_under_rust() {
    let (_, registry) = staged_fixture("continue-renewed", "renewed.json");
    let token = "gZrIqNVGgaTWd0eRH702NHHBNEKssae7a23IaAp0T8o";
    registry
        .complete("task-alpha", "agent-a", token, 1, &at(30))
        .unwrap();
    assert_eq!(board_shape(&registry, &at(40)), vec![]);
}

#[test]
fn fixture_takeover_fences_the_first_holder_under_rust() {
    let (_, registry) = staged_fixture("continue-taken", "taken-over.json");
    let first = "eIE6nDz4GwTmg1Xfbi_MClZ53yGokB0d5tJ58defsSA";
    let standing = "g8lm6gLOTd3ggd2Zon0D0hpNApNe9H6c3mfcEC9NWOY";
    for attempt in [
        registry.renew("task-alpha", "agent-a", first, 1, 30, &at(130)),
        registry
            .release("task-alpha", "agent-a", first, 1, &at(130))
            .map(|_| t0()),
        registry
            .complete("task-alpha", "agent-a", first, 1, &at(130))
            .map(|_| t0()),
    ] {
        assert!(matches!(attempt, Err(Error::Refusal(_))), "{attempt:?}");
    }
    registry
        .complete("task-alpha", "agent-b", standing, 2, &at(140))
        .unwrap();
    assert_eq!(board_shape(&registry, &at(150)), vec![]);
}

#[test]
fn fixture_mixed_board_expires_and_takes_over_under_rust() {
    let (_, registry) = staged_fixture("continue-mixed", "mixed.json");
    // An hour past the base instant both standing leases have
    // expired, so the board shows every unfinished item open.
    assert_eq!(
        board_shape(&registry, &at(3600)),
        vec![
            ("task-alpha".to_string(), "open".to_string(), None, 0, None),
            ("task-beta".to_string(), "open".to_string(), None, 1, None),
            ("task-gamma".to_string(), "open".to_string(), None, 1, None),
        ]
    );
    let granted = registry
        .claim("task-gamma", "agent-e", 60, &at(3600))
        .unwrap();
    assert_eq!(granted.epoch, 2);
    let events = registry.history(Some("task-gamma")).unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.kind, "taken_over");
    assert_eq!(last.previous_holder.as_deref(), Some("agent-c"));
    // The completed item keeps its history and refuses a claim.
    let delta = registry.history(Some("task-delta")).unwrap();
    assert_eq!(delta.last().unwrap().kind, "completed");
    assert!(matches!(
        registry.claim("task-delta", "agent-e", 60, &at(3600)),
        Err(Error::Refusal(_))
    ));
}

#[test]
fn fixture_offers_and_released_and_completed_states_continue() {
    let (_, offers) = staged_fixture("continue-offers", "offers.json");
    let granted = offers.claim("task-beta", "agent-z", 60, &at(10)).unwrap();
    assert_eq!(granted.epoch, 1);

    let (_, released) = staged_fixture("continue-released", "released.json");
    let granted = released
        .claim("task-alpha", "agent-b", 60, &at(40))
        .unwrap();
    assert_eq!(granted.epoch, 2);

    let (_, completed) = staged_fixture("continue-completed", "completed.json");
    assert!(matches!(
        completed.claim("task-alpha", "agent-b", 60, &at(40)),
        Err(Error::Refusal(_))
    ));

    let (_, empty) = staged_fixture("continue-empty", "empty.json");
    empty.offer("task-new", "fresh", &at(0)).unwrap();
    assert_eq!(board_shape(&empty, &at(1)).len(), 1);
}

#[test]
fn rust_written_offers_match_the_python_bytes() {
    let dir = temp_dir("byte-identity");
    let registry = Registry::new(dir.join("registry.json"));
    registry
        .offer("task-alpha", "first payload", &t0())
        .unwrap();
    registry
        .offer("task-beta", "second payload", &at(5))
        .unwrap();
    let written = std::fs::read(dir.join("registry.json")).unwrap();
    let frozen = std::fs::read(fixture_path("offers.json")).unwrap();
    assert_eq!(written, frozen);
}

#[test]
fn rust_written_claim_matches_the_python_document_field_by_field() {
    let dir = temp_dir("claim-parity");
    let registry = Registry::new(dir.join("registry.json"));
    registry
        .offer("task-alpha", "first payload", &t0())
        .unwrap();
    let granted = registry
        .claim("task-alpha", "agent-a", 600, &at(10))
        .unwrap();
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("registry.json")).unwrap()).unwrap();
    let frozen: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture_path("claimed.json")).unwrap()).unwrap();
    assert_eq!(written["history"], frozen["history"]);
    let mut written_item = written["items"][0].clone();
    let mut frozen_item = frozen["items"][0].clone();
    assert_eq!(
        written_item["token_hash"].as_str().unwrap(),
        dibs::token_hash(&granted.token)
    );
    written_item
        .as_object_mut()
        .unwrap()
        .insert("token_hash".to_string(), serde_json::Value::Null);
    frozen_item
        .as_object_mut()
        .unwrap()
        .insert("token_hash".to_string(), serde_json::Value::Null);
    assert_eq!(written_item, frozen_item);
}

#[test]
fn stored_epoch_at_the_unsigned_top_still_stands() {
    let dir = temp_dir("epoch-top");
    let path = dir.join("registry.json");
    std::fs::write(
        &path,
        "{\"format\": \"dibs-registry/1\", \"items\": [{\"id\": \"task-one\", \
         \"note\": \"\", \"status\": \"open\", \"holder\": null, \"token_hash\": null, \
         \"epoch\": 18446744073709551615, \"expires_at\": null}], \"history\": []}",
    )
    .unwrap();
    let registry = Registry::new(path);
    let rows = registry.board(&t0()).unwrap();
    assert_eq!(rows[0].epoch, 18446744073709551615i128);
}

#[test]
fn epoch_increment_is_checked_at_the_magnitudes() {
    let dir = temp_dir("epoch-increment");
    let path = dir.join("registry.json");
    std::fs::write(
        &path,
        "{\"format\": \"dibs-registry/1\", \"items\": [{\"id\": \"task-one\", \
         \"note\": \"\", \"status\": \"open\", \"holder\": null, \"token_hash\": null, \
         \"epoch\": 999999999999999999, \"expires_at\": null}], \"history\": []}",
    )
    .unwrap();
    let registry = Registry::new(path);
    let granted = registry.claim("task-one", "agent-a", 60, &t0()).unwrap();
    assert_eq!(granted.epoch, 1000000000000000000i128);
    let rows = registry.board(&t0()).unwrap();
    assert_eq!(rows[0].epoch, 1000000000000000000i128);
}

#[test]
fn claim_at_the_unsigned_top_is_refused_and_changes_nothing() {
    let text = "{\"format\": \"dibs-registry/1\", \"items\": [{\"id\": \"task-one\", \
         \"note\": \"\", \"status\": \"open\", \"holder\": null, \"token_hash\": null, \
         \"epoch\": 18446744073709551615, \"expires_at\": null}], \"history\": []}";
    let (dir, registry) = staged_document("epoch-top-claim", text);
    let error = expect_error(registry.claim("task-one", "agent-a", 60, &t0()));
    assert!(matches!(error, Error::Refusal(_)), "{error:?}");
    assert!(
        error.message().contains("18446744073709551615"),
        "{}",
        error.message()
    );
    assert!(
        error.message().contains("no epoch past it can be assigned"),
        "{}",
        error.message()
    );
    assert_eq!(
        std::fs::read(dir.join("registry.json")).unwrap(),
        text.as_bytes()
    );
}

#[test]
fn claim_below_the_unsigned_top_assigns_the_top_and_then_fences() {
    let text = "{\"format\": \"dibs-registry/1\", \"items\": [{\"id\": \"task-one\", \
         \"note\": \"\", \"status\": \"open\", \"holder\": null, \"token_hash\": null, \
         \"epoch\": 18446744073709551614, \"expires_at\": null}], \"history\": []}";
    let (_, registry) = staged_document("epoch-top-minus-one", text);
    let granted = registry.claim("task-one", "agent-a", 60, &t0()).unwrap();
    assert_eq!(granted.epoch, 18446744073709551615i128);
    let rows = registry.board(&t0()).unwrap();
    assert_eq!(rows[0].epoch, 18446744073709551615i128);
    registry
        .release(
            "task-one",
            "agent-a",
            &granted.token,
            18446744073709551615i128,
            &at(1),
        )
        .unwrap();
    let error = expect_error(registry.claim("task-one", "agent-b", 60, &at(2)));
    assert!(matches!(error, Error::Refusal(_)), "{error:?}");
    let rows = registry.board(&at(3)).unwrap();
    assert_eq!(rows[0].status, "open");
    assert_eq!(rows[0].epoch, 18446744073709551615i128);
}

#[test]
fn renewal_past_the_utc_edge_is_refused_and_changes_nothing() {
    // The standing expiry reads 22:30 at an offset of minus one
    // hour, which is 23:30 in UTC. One more hour of lease lands
    // in UTC year 10000, outside the representable range, even
    // though the wall clock year stays 9999.
    let text = claimed_document("9999-12-31T22:30:00-01:00");
    let (dir, registry) = staged_document("utc-edge-renewal", &text);
    let now = Instant::from_civil(9999, 12, 31, 21, 0, 0, 0, Some(-3_600_000_000)).unwrap();
    let error =
        expect_error(registry.renew("task-one", "agent-a", "standing-token", 1, 3600, &now));
    assert!(matches!(error, Error::InvalidTtl(_)), "{error:?}");
    assert_eq!(
        std::fs::read(dir.join("registry.json")).unwrap(),
        text.as_bytes()
    );
}

#[test]
fn a_now_outside_the_utc_range_is_refused() {
    // Midnight opening year 1 at an offset of plus one hour is
    // 23:00 on the last day of year 0 in UTC, and half past
    // eleven closing year 9999 at an offset of minus one hour
    // is year 10000 in UTC. Neither instant can be recorded.
    let dir = temp_dir("utc-range-now");
    let registry = Registry::new(dir.join("registry.json"));
    let early = Instant::from_civil(1, 1, 1, 0, 0, 0, 0, Some(3_600_000_000)).unwrap();
    let error = expect_error(registry.offer("task-one", "a synthetic task", &early));
    assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
    let error = expect_error(registry.board(&early));
    assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
    let late = Instant::from_civil(9999, 12, 31, 23, 30, 0, 0, Some(-3_600_000_000)).unwrap();
    let error = expect_error(registry.offer("task-one", "a synthetic task", &late));
    assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
    // The same wall clock fields in UTC itself stand inside the
    // range and work.
    registry
        .offer("task-one", "a synthetic task", &t0())
        .unwrap();
}

#[test]
fn renewal_straddling_the_year_line_inside_utc_still_stands() {
    // The standing expiry reads 22:30 at an offset of plus one
    // hour, which is 21:30 in UTC. One more hour lands at 22:30
    // in UTC, inside the range, and the renewal stands.
    let text = claimed_document("9999-12-31T22:30:00+01:00");
    let (_, registry) = staged_document("utc-edge-control", &text);
    let now = Instant::from_civil(9999, 12, 31, 20, 0, 0, 0, Some(0)).unwrap();
    let expires = registry
        .renew("task-one", "agent-a", "standing-token", 1, 3600, &now)
        .unwrap();
    assert_eq!(dibs::format_instant(&expires), "9999-12-31T22:30:00+00:00");
    let rows = registry.board(&now).unwrap();
    assert_eq!(
        rows[0].expires_at.as_deref(),
        Some("9999-12-31T22:30:00+00:00")
    );
}

#[test]
fn stored_week_date_spellings_read_at_their_boundary() {
    // Week dates in the family the Python reader accepted, the
    // basic form with a weekday, the basic form without one,
    // and the extended form. Each names its expiry, and the
    // lease is live strictly before it and expired at it.
    let spellings: [(&str, (i64, i64, i64)); 3] = [
        ("2031W183T12:01:00+00:00", (2031, 4, 30)),
        ("2031W18T12:01:00+00:00", (2031, 4, 28)),
        ("2031-W18-3T12:01:00+00:00", (2031, 4, 30)),
    ];
    for (index, (spelling, (year, month, day))) in spellings.iter().enumerate() {
        let text = claimed_document(spelling);
        let (_, registry) = staged_document(&format!("week-date-{index}"), &text);
        let boundary = Instant::from_civil(*year, *month, *day, 12, 1, 0, 0, Some(0)).unwrap();
        let live = registry.board(&boundary.plus_seconds(-1)).unwrap();
        assert_eq!(live[0].status, "claimed", "{spelling}");
        assert_eq!(live[0].expires_at.as_deref(), Some(*spelling));
        let expired = registry.board(&boundary).unwrap();
        assert_eq!(expired[0].status, "open", "{spelling}");
    }
}

/// A generated registry of the given number of offered items,
/// written out as text the way the scale cases stage it.
fn generated_document(count: usize) -> String {
    let mut out = String::from("{\"format\": \"dibs-registry/1\", \"items\": [");
    for index in 0..count {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"id\": \"item-{index:06}\", \"note\": \"\", \"status\": \"open\", \
             \"holder\": null, \"token_hash\": null, \"epoch\": 0, \"expires_at\": null}}"
        ));
    }
    out.push_str("], \"history\": [");
    for index in 0..count {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"seq\": {}, \"kind\": \"offered\", \"item\": \"item-{index:06}\", \
             \"holder\": null, \"at\": \"2031-05-04T12:00:00+00:00\", \"epoch\": 0, \
             \"previous_holder\": null}}",
            index + 1
        ));
    }
    out.push_str("]}");
    out
}

#[test]
fn board_over_a_large_registry_stays_inside_a_generous_bound() {
    // The port as first built validated a document in time
    // quadratic in the item count, and a board over sixteen
    // thousand items took over seventeen seconds in this
    // profile on the build machine. Validation is linear in
    // the count, and the same board lands far inside the bound.
    let count = 16_000usize;
    let text = generated_document(count);
    let (_, registry) = staged_document("scale-board", &text);
    let started = std::time::Instant::now();
    let rows = registry.board(&t0()).unwrap();
    let elapsed = started.elapsed();
    assert_eq!(rows.len(), count);
    assert_eq!(rows[0].id, "item-000000");
    assert_eq!(rows[count - 1].id, "item-015999");
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "board over {count} items took {elapsed:?}"
    );
}

#[test]
fn offset_at_the_i64_edges_is_refused_not_panicked() {
    assert!(Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(i64::MIN)).is_none());
    assert!(Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(i64::MAX)).is_none());
    assert!(Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(86_400_000_000)).is_none());
    assert!(Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(-86_400_000_000)).is_none());
    // The largest offsets inside a day still build.
    assert!(Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(86_399_999_999)).is_some());
    assert!(Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(-86_399_999_999)).is_some());
}

#[test]
fn epoch_micros_refuses_counts_it_cannot_hold() {
    // A count naming a day count past the width the type holds
    // is refused, where the port as first built wrapped it and
    // read back a different count.
    assert!(Instant::from_epoch_micros(i128::MAX).is_none());
    assert!(Instant::from_epoch_micros(i128::MIN).is_none());
    let counts = [
        0i128,
        -1,
        t0().epoch_micros().unwrap(),
        Instant::from_civil(9999, 12, 31, 23, 59, 59, 999999, Some(0))
            .unwrap()
            .epoch_micros()
            .unwrap(),
        Instant::from_civil(1, 1, 1, 0, 0, 0, 0, Some(0))
            .unwrap()
            .epoch_micros()
            .unwrap(),
    ];
    for count in counts {
        let built = Instant::from_epoch_micros(count).unwrap();
        assert_eq!(built.epoch_micros(), Some(count));
    }
}

#[test]
fn format_instant_renders_naive_and_aware_instants() {
    let naive = Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, None).unwrap();
    assert_eq!(dibs::format_instant(&naive), "2031-05-04T12:00:00");
    let naive_micros = Instant::from_civil(2031, 5, 4, 12, 0, 0, 123456, None).unwrap();
    assert_eq!(
        dibs::format_instant(&naive_micros),
        "2031-05-04T12:00:00.123456"
    );
    assert_eq!(dibs::format_instant(&t0()), "2031-05-04T12:00:00+00:00");
}

#[test]
fn aware_instants_compare_by_the_instant_not_the_offset() {
    let utc = Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(0)).unwrap();
    let shifted = Instant::from_civil(2031, 5, 4, 13, 0, 0, 0, Some(3_600_000_000)).unwrap();
    assert_eq!(utc, shifted);
    assert_eq!(utc.cmp(&shifted), std::cmp::Ordering::Equal);
    let naive = Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, None).unwrap();
    assert_ne!(utc, naive);
}

#[test]
fn stored_instants_in_the_wider_family_read_at_their_boundary() {
    // The stored instant family the Python reader accepted
    // reaches past the canonical rendering: a space separator, a
    // comma fraction, a Z, and a numeric offset all name the same
    // expiry, and the lease is live strictly before it and
    // expired at it under each spelling.
    let spellings = [
        "2031-05-04 12:01:00+00:00",
        "2031-05-04T12:01:00,000000+00:00",
        "2031-05-04T12:01:00Z",
        "2031-05-04T13:01:00+01:00",
        "20310504T120100+0000",
    ];
    for (index, spelling) in spellings.iter().enumerate() {
        let dir = temp_dir(&format!("instant-family-{index}"));
        let path = dir.join("registry.json");
        let text = format!(
            "{{\"format\": \"dibs-registry/1\", \"items\": [{{\"id\": \"task-one\", \
             \"note\": \"\", \"status\": \"claimed\", \"holder\": \"agent-a\", \
             \"token_hash\": \"{}\", \"epoch\": 1, \"expires_at\": \"{}\"}}], \
             \"history\": []}}",
            "a".repeat(64),
            spelling
        );
        std::fs::write(&path, text).unwrap();
        let registry = Registry::new(path);
        let live = registry.board(&at(59)).unwrap();
        assert_eq!(live[0].status, "claimed", "{spelling}");
        // The board reports the stored spelling whole.
        assert_eq!(live[0].expires_at.as_deref(), Some(*spelling));
        let boundary = registry.board(&at(60)).unwrap();
        assert_eq!(boundary[0].status, "open", "{spelling}");
    }
}
