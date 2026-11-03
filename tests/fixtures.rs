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
