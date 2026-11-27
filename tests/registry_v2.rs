//! Integration pins for slice DB-S3 (`dibs-registry/2` DAG dependencies,
//! priorities, capability tags, bounded takeovers with reset, ready queries,
//! and crash-idempotent history compaction under Obligations DB-21..DB-24).

use std::path::{Path, PathBuf};
use std::process::Command;

use dibs::{Error, Instant, OfferOptions, OfferSpec, Registry, Value};

mod common;
use common::{at, expect_error, sha256_hex, t0};

fn temp_dir(tag: &str) -> PathBuf {
    common::temp_dir("dibs-v2-test-", tag)
}

struct CliResult {
    stdout: String,
    stderr: String,
}

fn run_cli(args: &[&str], stdin_bytes: Option<&[u8]>, expected_code: i32) -> CliResult {
    use std::io::Write;
    use std::process::Stdio;

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dibs"));
    cmd.args(args);
    cmd.env_remove("DIBS_REGISTRY");
    cmd.env_remove("DIBS_TOKEN");
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    if stdin_bytes.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    let mut child = cmd.spawn().unwrap();
    if let Some(input) = stdin_bytes {
        let mut handle = child.stdin.take().unwrap();
        let _ = handle.write_all(input);
    }
    let out = child.wait_with_output().unwrap();
    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(
        code, expected_code,
        "args={args:?}\nstdout={stdout}\nstderr={stderr}"
    );
    CliResult { stdout, stderr }
}

fn object_keys(value: &Value) -> Vec<&str> {
    let Value::Object(pairs) = value else {
        panic!("expected JSON object, got {value:?}");
    };
    pairs.iter().map(|(k, _)| k.as_str()).collect()
}

fn object_field<'a>(value: &'a Value, name: &str) -> &'a Value {
    let Value::Object(pairs) = value else {
        panic!("expected JSON object, got {value:?}");
    };
    pairs
        .iter()
        .find_map(|(k, v)| (k == name).then_some(v))
        .unwrap_or_else(|| panic!("missing field {name}"))
}

fn assert_corrupt_and_unchanged(path: &Path, raw: &str) -> Error {
    std::fs::write(path, raw).unwrap();
    let registry = Registry::new(path.to_path_buf());
    let err = expect_error(registry.board(&t0()));
    assert!(
        matches!(err, Error::Corrupt(_)),
        "expected Corrupt on {raw}, got {err:?}"
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), raw);
    err
}

#[test]
fn v1_byte_identity_holds_when_v2_features_stand_unused() {
    let dir = temp_dir("v1-byte-identity");
    let path = dir.join("registry.json");
    let registry = Registry::new(path.clone());

    // Default OfferOptions / OfferSpec values match dibs-registry/1 defaults.
    let spec_a = OfferOptions::new("alpha", "first synthetic item");
    registry.offer_with(&spec_a, &t0()).unwrap();
    registry
        .offer_many(
            &[
                OfferSpec::new("beta", ""),
                OfferSpec::new("gamma", "third synthetic item"),
            ],
            &at(5),
        )
        .unwrap();

    let raw_after_offers = std::fs::read_to_string(&path).unwrap();
    let parsed: Value = serde_json::from_str(&raw_after_offers).unwrap();
    assert_eq!(object_keys(&parsed), vec!["format", "items", "history"]);
    assert_eq!(
        object_field(&parsed, "format"),
        &Value::Str("dibs-registry/1".to_string())
    );
    let Value::Array(items) = object_field(&parsed, "items") else {
        panic!("expected items array");
    };
    assert_eq!(items.len(), 3);
    for item in items {
        assert_eq!(
            object_keys(item),
            vec![
                "id",
                "note",
                "status",
                "holder",
                "token_hash",
                "epoch",
                "expires_at"
            ]
        );
    }

    // Full v1 lifecycle (claim, renew, release, takeover, complete) keeps dibs-registry/1.
    let g1 = registry.claim("alpha", "agent-a", 60, &at(10)).unwrap();
    registry
        .renew("alpha", "agent-a", &g1.token, 1, 30, &at(20))
        .unwrap();
    let g2 = registry.claim("alpha", "agent-b", 60, &at(100)).unwrap();
    assert_eq!(g2.epoch, 2);
    registry
        .complete("alpha", "agent-b", &g2.token, 2, &at(110))
        .unwrap();
    let g_beta = registry
        .claim_next_with_tags("agent-c", 60, &[], &at(115))
        .unwrap();
    assert_eq!(g_beta.item, "beta");
    registry
        .release("beta", "agent-c", &g_beta.token, 1, &at(120))
        .unwrap();

    let raw_final = std::fs::read_to_string(&path).unwrap();
    let parsed_final: Value = serde_json::from_str(&raw_final).unwrap();
    assert_eq!(
        object_keys(&parsed_final),
        vec!["format", "items", "history"]
    );
    assert_eq!(
        object_field(&parsed_final, "format"),
        &Value::Str("dibs-registry/1".to_string())
    );

    // Exact fixture byte-for-byte match against tests/fixtures/offers.json.
    let fix_dir = temp_dir("v1-fixture-exact");
    let fix_path = fix_dir.join("offers.json");
    let fix_reg = Registry::new(fix_path.clone());
    fix_reg
        .offer_with(&OfferSpec::new("task-alpha", "first payload"), &t0())
        .unwrap();
    fix_reg
        .offer_with(&OfferSpec::new("task-beta", "second payload"), &at(5))
        .unwrap();
    let expected_fixture = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/offers.json"),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&fix_path).unwrap(),
        expected_fixture
    );
}

#[test]
fn v2_promotion_triggers_and_stays_sticky_across_subsequent_mutations() {
    let expected_doc_keys = vec!["format", "compaction", "items", "history"];
    let expected_item_keys = vec![
        "id",
        "note",
        "status",
        "priority",
        "tags",
        "depends_on",
        "max_takeovers",
        "takeover_count",
        "holder",
        "token_hash",
        "epoch",
        "expires_at",
    ];

    // Trigger 1: non-default priority (0, 1, 3)
    for prio in [0u8, 1, 3] {
        let dir = temp_dir(&format!("v2-promo-prio-{prio}"));
        let path = dir.join("registry.json");
        let reg = Registry::new(path.clone());
        reg.offer("base-v1", "", &t0()).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"dibs-registry/1\""));
        reg.offer_with(&OfferSpec::new("prio-item", "").with_priority(prio), &at(1))
            .unwrap();
        let val: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(object_keys(&val), expected_doc_keys);
        assert_eq!(
            object_field(&val, "format"),
            &Value::Str("dibs-registry/2".to_string())
        );
        assert_eq!(object_field(&val, "compaction"), &Value::Null);
        let Value::Array(items) = object_field(&val, "items") else {
            panic!("expected items");
        };
        for item in items {
            assert_eq!(object_keys(item), expected_item_keys);
        }
        // Sticky check: offering a plain v1-shaped item and completing prio-item keeps /2!
        let g = reg.claim("prio-item", "worker-1", 60, &at(2)).unwrap();
        reg.complete("prio-item", "worker-1", &g.token, 1, &at(3))
            .unwrap();
        reg.offer("plain-after", "", &at(4)).unwrap();
        let after_val: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            object_field(&after_val, "format"),
            &Value::Str("dibs-registry/2".to_string())
        );
    }

    // Trigger 2: non-empty tags
    {
        let dir = temp_dir("v2-promo-tags");
        let path = dir.join("registry.json");
        let reg = Registry::new(path.clone());
        reg.offer_with(
            &OfferSpec::new("tagged", "").with_tags(vec!["gpu".to_string()]),
            &t0(),
        )
        .unwrap();
        let val: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            object_field(&val, "format"),
            &Value::Str("dibs-registry/2".to_string())
        );
    }

    // Trigger 3: non-empty depends_on
    {
        let dir = temp_dir("v2-promo-deps");
        let path = dir.join("registry.json");
        let reg = Registry::new(path.clone());
        reg.offer("first", "", &t0()).unwrap();
        reg.offer_with(
            &OfferSpec::new("second", "").with_depends_on(vec!["first".to_string()]),
            &at(1),
        )
        .unwrap();
        let val: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            object_field(&val, "format"),
            &Value::Str("dibs-registry/2".to_string())
        );
    }

    // Trigger 4: max_takeovers = Some(_)
    {
        let dir = temp_dir("v2-promo-cap");
        let path = dir.join("registry.json");
        let reg = Registry::new(path.clone());
        reg.offer_with(
            &OfferSpec::new("capped", "").with_max_takeovers(Some(3)),
            &t0(),
        )
        .unwrap();
        let val: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            object_field(&val, "format"),
            &Value::Str("dibs-registry/2".to_string())
        );
    }

    // Trigger 5: compact promotes a dibs-registry/1 document to dibs-registry/2
    {
        let dir = temp_dir("v2-promo-compact");
        let path = dir.join("registry.json");
        let archive = dir.join("archive.jsonl");
        let reg = Registry::new(path.clone());
        reg.offer("only-v1", "", &t0()).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"dibs-registry/1\""));
        reg.compact(1, &archive, &at(10)).unwrap();
        let val: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(object_keys(&val), expected_doc_keys);
        assert_eq!(
            object_field(&val, "format"),
            &Value::Str("dibs-registry/2".to_string())
        );
        let comp_obj = object_field(&val, "compaction");
        assert_eq!(
            object_keys(comp_obj),
            vec![
                "through_seq",
                "archived_events_sha256",
                "compacted_at",
                "baseline",
            ]
        );
        let Value::Array(base_arr) = object_field(comp_obj, "baseline") else {
            panic!("expected baseline array");
        };
        assert_eq!(base_arr.len(), 1);
        assert_eq!(
            object_keys(&base_arr[0]),
            vec!["id", "status", "holder", "epoch", "takeover_count"]
        );
    }

    // Pure sticky `/2` check: stage a valid `/2` registry where ALL items have default
    // `/1`-compatible metadata (`priority: 2, tags: [], depends_on: [], max_takeovers: null`),
    // `compaction: null`, and zero `reset` events, then call plain `offer` and verify `/2` sticks!
    {
        let dir = temp_dir("v2-promo-pure-sticky");
        let path = dir.join("registry.json");
        let pure_v2 = serde_json::json!({
            "format": "dibs-registry/2",
            "compaction": null,
            "items": [{
                "id": "default-item",
                "note": "",
                "status": "open",
                "priority": 2,
                "tags": [],
                "depends_on": [],
                "max_takeovers": null,
                "takeover_count": 0,
                "holder": null,
                "token_hash": null,
                "epoch": 0,
                "expires_at": null
            }],
            "history": [{
                "seq": 1,
                "kind": "offered",
                "item": "default-item",
                "holder": null,
                "at": "2031-05-04T12:00:00+00:00",
                "epoch": 0,
                "previous_holder": null
            }]
        });
        std::fs::write(&path, serde_json::to_string(&pure_v2).unwrap()).unwrap();
        let reg = Registry::new(path.clone());
        reg.offer("plain-next", "", &at(1)).unwrap();
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            object_field(&after, "format"),
            &Value::Str("dibs-registry/2".to_string())
        );
    }
}

#[test]
fn v2_corruption_battery_rejects_every_schema_and_cross_validation_tamper() {
    let dir = temp_dir("v2-corrupt-battery");
    let seed_path = dir.join("seed.json");
    let seed_archive = dir.join("seed-archive.jsonl");
    let seed_reg = Registry::new(seed_path.clone());

    seed_reg
        .offer_with(
            &OfferSpec::new("item-a", "note a")
                .with_priority(1)
                .with_tags(vec!["alpha".to_string(), "beta".to_string()])
                .with_max_takeovers(Some(2)),
            &t0(),
        )
        .unwrap();
    seed_reg
        .offer_with(
            &OfferSpec::new("item-b", "")
                .with_depends_on(vec!["item-a".to_string()])
                .with_max_takeovers(Some(1)),
            &at(1),
        )
        .unwrap();
    let _g1 = seed_reg.claim("item-a", "alice", 10, &at(2)).unwrap();
    let g2 = seed_reg.claim("item-a", "bob", 10, &at(20)).unwrap();
    // Compact at seq 4 while item-a stands at epoch 2, takeover_count 1!
    seed_reg.compact(4, &seed_archive, &at(22)).unwrap();
    seed_reg
        .complete("item-a", "bob", &g2.token, 2, &at(25))
        .unwrap();
    seed_reg.offer("item-c", "post compact", &at(35)).unwrap();

    let valid_v2_raw = std::fs::read_to_string(&seed_path).unwrap();
    assert_eq!(seed_reg.board(&at(40)).unwrap().len(), 2);

    let check_path = dir.join("tampered.json");
    let mutate_json_msg = |f: &dyn Fn(&mut serde_json::Value), expected_substr: &str| -> Error {
        let mut doc: serde_json::Value = serde_json::from_str(&valid_v2_raw).unwrap();
        f(&mut doc);
        let raw = serde_json::to_string(&doc).unwrap();
        let err = assert_corrupt_and_unchanged(&check_path, &raw);
        assert!(
            err.message().contains(expected_substr),
            "expected substring {expected_substr:?} in {:?}",
            err.message()
        );
        err
    };

    // 1. Foreign format string names both v1 and v2, even when foreign root keys are present!
    let err = mutate_json_msg(
        &|doc| doc["format"] = serde_json::json!("dibs-registry/99"),
        "not 'dibs-registry/1' or 'dibs-registry/2'",
    );
    assert_eq!(
        err.message(),
        "the registry is corrupt, the format name is 'dibs-registry/99', \
         not 'dibs-registry/1' or 'dibs-registry/2'"
    );
    let foreign_with_extra =
        "{\"format\": \"dibs-registry/99\", \"extra\": 1, \"items\": [], \"history\": []}";
    let err_foreign_extra = assert_corrupt_and_unchanged(&check_path, foreign_with_extra);
    assert_eq!(
        err_foreign_extra.message(),
        "the registry is corrupt, the format name is 'dibs-registry/99', \
         not 'dibs-registry/1' or 'dibs-registry/2'"
    );

    // 2. Root missing compaction or carrying extra field or duplicate key.
    mutate_json_msg(
        &|doc| {
            doc.as_object_mut().unwrap().remove("compaction");
        },
        "the document does not carry exactly the recorded fields",
    );
    mutate_json_msg(
        &|doc| {
            doc["extra_root"] = serde_json::json!(1);
        },
        "the document does not carry exactly the recorded fields",
    );
    let dup_root = valid_v2_raw.replacen(
        "\"format\": \"dibs-registry/2\"",
        "\"format\": \"dibs-registry/2\", \"format\": \"dibs-registry/2\"",
        1,
    );
    let dup_err = assert_corrupt_and_unchanged(&check_path, &dup_root);
    assert!(dup_err
        .message()
        .contains("a key stands twice in the document, format"));

    // 3. Item schema damages: every case asserts its exact validator message!
    mutate_json_msg(
        &|doc| {
            doc["items"][0]
                .as_object_mut()
                .unwrap()
                .remove("takeover_count");
        },
        "an item does not carry exactly the recorded fields",
    );
    mutate_json_msg(
        &|doc| {
            doc["items"][0]["extra_item"] = serde_json::json!(null);
        },
        "an item does not carry exactly the recorded fields",
    );
    for bad_prio in [
        serde_json::json!(-1),
        serde_json::json!(4),
        serde_json::json!("2"),
        serde_json::json!(true),
    ] {
        mutate_json_msg(
            &|doc| doc["items"][0]["priority"] = bad_prio.clone(),
            "holds an invalid priority",
        );
    }
    mutate_json_msg(
        &|doc| doc["items"][0]["tags"] = serde_json::json!(["beta", "alpha"]),
        "holds unsorted or duplicate tags",
    );
    mutate_json_msg(
        &|doc| doc["items"][0]["tags"] = serde_json::json!(["alpha", "alpha"]),
        "holds unsorted or duplicate tags",
    );
    mutate_json_msg(
        &|doc| doc["items"][0]["tags"] = serde_json::json!(["Bad Tag"]),
        "invalid tag id 'Bad Tag'",
    );
    mutate_json_msg(
        &|doc| {
            let seventeen: Vec<String> = (0..17).map(|i| format!("tag-{i:02}")).collect();
            doc["items"][0]["tags"] = serde_json::json!(seventeen);
        },
        "holds an invalid tags list",
    );
    mutate_json_msg(
        &|doc| doc["items"][1]["depends_on"] = serde_json::json!(["item-b"]),
        "item 'item-b' depends on itself",
    );
    mutate_json_msg(
        &|doc| doc["items"][1]["depends_on"] = serde_json::json!(["item-c", "item-a"]),
        "holds unsorted or duplicate dependencies",
    );
    mutate_json_msg(
        &|doc| doc["items"][1]["depends_on"] = serde_json::json!(["item-a", "item-a"]),
        "holds unsorted or duplicate dependencies",
    );
    mutate_json_msg(
        &|doc| doc["items"][1]["depends_on"] = serde_json::json!(["ghost-item"]),
        "depends on 'ghost-item', which does not stand in the backlog",
    );
    mutate_json_msg(
        &|doc| {
            let sixty_five: Vec<String> = (0..65).map(|i| format!("dep-{i:02}")).collect();
            doc["items"][1]["depends_on"] = serde_json::json!(sixty_five);
        },
        "holds an invalid depends_on list",
    );
    // Also stage 65 REAL standing prerequisite items so unknown-target validation could never mask the 64-cap!
    {
        let mut real65_items = Vec::new();
        let mut real65_deps = Vec::new();
        for i in 0..65 {
            let dep_id = format!("dep-{i:02}");
            real65_deps.push(dep_id.clone());
            real65_items.push(serde_json::json!({
                "id": dep_id,
                "note": "",
                "status": "open",
                "priority": 2,
                "tags": [],
                "depends_on": [],
                "max_takeovers": null,
                "takeover_count": 0,
                "holder": null,
                "token_hash": null,
                "epoch": 0,
                "expires_at": null
            }));
        }
        real65_items.push(serde_json::json!({
            "id": "fan-in-65",
            "note": "",
            "status": "open",
            "priority": 2,
            "tags": [],
            "depends_on": real65_deps,
            "max_takeovers": null,
            "takeover_count": 0,
            "holder": null,
            "token_hash": null,
            "epoch": 0,
            "expires_at": null
        }));
        let doc65 = serde_json::json!({
            "format": "dibs-registry/2",
            "compaction": null,
            "items": real65_items,
            "history": []
        });
        let err65 =
            assert_corrupt_and_unchanged(&check_path, &serde_json::to_string(&doc65).unwrap());
        assert!(err65.message().contains("holds an invalid depends_on list"));
    }
    // Cycle on disk: item-a depends on item-b while item-b depends on item-a.
    mutate_json_msg(
        &|doc| doc["items"][0]["depends_on"] = serde_json::json!(["item-b"]),
        "the item dependency graph contains a cycle",
    );

    for bad_cap in [
        serde_json::json!(0),
        serde_json::json!(1001),
        serde_json::json!("2"),
        serde_json::json!(false),
    ] {
        mutate_json_msg(
            &|doc| doc["items"][0]["max_takeovers"] = bad_cap.clone(),
            "holds an invalid max_takeovers",
        );
    }
    mutate_json_msg(
        &|doc| doc["items"][0]["takeover_count"] = serde_json::json!(-1),
        "holds an invalid takeover_count",
    );
    mutate_json_msg(
        &|doc| doc["items"][0]["takeover_count"] = serde_json::json!(2),
        "holds takeover_count 2 at epoch 2",
    );
    mutate_json_msg(
        &|doc| doc["items"][1]["takeover_count"] = serde_json::json!(1),
        "holds takeover_count 1 at epoch 0",
    );
    // Unmasked `takeover_count > max_takeovers` check: epoch is 4 (so 3 < 4 passes epoch check) while max_takeovers is 2!
    {
        let over_cap_doc = serde_json::json!({
            "format": "dibs-registry/2",
            "compaction": null,
            "items": [{
                "id": "over-cap",
                "note": "",
                "status": "open",
                "priority": 2,
                "tags": [],
                "depends_on": [],
                "max_takeovers": 2,
                "takeover_count": 3,
                "holder": null,
                "token_hash": null,
                "epoch": 4,
                "expires_at": null
            }],
            "history": []
        });
        let err_cap = assert_corrupt_and_unchanged(
            &check_path,
            &serde_json::to_string(&over_cap_doc).unwrap(),
        );
        assert!(
            err_cap
                .message()
                .contains("item 'over-cap' holds takeover_count 3 past max_takeovers 2"),
            "{}",
            err_cap.message()
        );
    }

    // 4. Compaction object & baseline schema damages.
    mutate_json_msg(
        &|doc| doc["compaction"]["through_seq"] = serde_json::json!(0),
        "compaction holds an invalid through_seq",
    );
    mutate_json_msg(
        &|doc| doc["compaction"]["through_seq"] = serde_json::json!(-5),
        "compaction holds an invalid through_seq",
    );
    mutate_json_msg(
        &|doc| {
            doc["compaction"]["archived_events_sha256"] = serde_json::json!("A".repeat(64));
        },
        "compaction holds an invalid archived_events_sha256",
    );
    mutate_json_msg(
        &|doc| {
            doc["compaction"]["archived_events_sha256"] = serde_json::json!("a".repeat(63));
        },
        "compaction holds an invalid archived_events_sha256",
    );
    mutate_json_msg(
        &|doc| {
            doc["compaction"]["compacted_at"] = serde_json::json!("2031-05-04T12:00:00");
        },
        "an instant field holds a naive instant",
    );
    mutate_json_msg(
        &|doc| {
            doc["compaction"]["extra"] = serde_json::json!(1);
        },
        "the compaction field does not carry exactly the recorded fields",
    );
    mutate_json_msg(
        &|doc| {
            doc["compaction"]["baseline"][0]["extra"] = serde_json::json!(1);
        },
        "a compaction baseline entry does not carry exactly the recorded fields",
    );
    mutate_json_msg(
        &|doc| {
            let arr = doc["compaction"]["baseline"].as_array_mut().unwrap();
            arr.reverse();
        },
        "the compaction baseline is not sorted without duplicates",
    );
    mutate_json_msg(
        &|doc| {
            doc["compaction"]["baseline"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "id": "z-missing",
                    "status": "open",
                    "holder": null,
                    "epoch": 0,
                    "takeover_count": 0
                }));
        },
        "the compaction baseline names item 'z-missing', which does not stand in the backlog",
    );
    mutate_json_msg(
        &|doc| {
            doc["compaction"]["baseline"][1]["status"] = serde_json::json!("exhausted");
        },
        "holds an unknown status 'exhausted'",
    );
    // Tampering baseline[0]["takeover_count"] from 1 -> 0 passes validate_compaction (0 < epoch 2)
    // and is caught directly by DB-7 baseline-to-items cross-validation!
    mutate_json_msg(
        &|doc| {
            doc["compaction"]["baseline"][0]["takeover_count"] = serde_json::json!(0);
        },
        "item 'item-a' stands at takeover_count 1 where its history ends at takeover_count 0",
    );
    mutate_json_msg(
        &|doc| {
            doc["items"][0]["takeover_count"] = serde_json::json!(0);
        },
        "item 'item-a' stands at takeover_count 0 where its history ends at takeover_count 1",
    );
    mutate_json_msg(
        &|doc| {
            doc["history"][0]["seq"] = serde_json::json!(99);
        },
        "the history sequence does not grow by 1 from 1 without gaps",
    );
    mutate_json_msg(
        &|doc| {
            doc["history"]
                .as_array_mut()
                .unwrap()
                .retain(|ev| ev["item"] != "item-c");
        },
        "item 'item-c' is absent from the compaction baseline and has no post-compaction history",
    );
    mutate_json_msg(
        &|doc| {
            let last_idx = doc["history"].as_array().unwrap().len() - 1;
            doc["history"][last_idx]["kind"] = serde_json::json!("claimed");
            doc["history"][last_idx]["holder"] = serde_json::json!("alice");
            doc["history"][last_idx]["epoch"] = serde_json::json!(1);
            doc["items"][2]["status"] = serde_json::json!("claimed");
            doc["items"][2]["holder"] = serde_json::json!("alice");
            doc["items"][2]["token_hash"] = serde_json::json!("a".repeat(64));
            doc["items"][2]["epoch"] = serde_json::json!(1);
            doc["items"][2]["expires_at"] = serde_json::json!("2031-05-04T13:00:00+00:00");
        },
        "item 'item-c' is absent from the compaction baseline and does not open its post-compaction history with offered",
    );

    // 5. Exhausted `claimed` without `reset` AND mid-life `reset` validation damages:
    // 5a. "reset" event in dibs-registry/1 is rejected as unknown event kind.
    let v1_with_reset = concat!(
        "{\"format\": \"dibs-registry/1\", ",
        "\"items\": [{\"id\": \"x\", \"note\": \"\", \"status\": \"open\", ",
        "\"holder\": null, \"token_hash\": null, \"epoch\": 2, \"expires_at\": null}], ",
        "\"history\": [",
        "{\"seq\": 1, \"kind\": \"offered\", \"item\": \"x\", \"holder\": null, \"at\": \"2031-05-04T12:00:00+00:00\", \"epoch\": 0, \"previous_holder\": null},",
        "{\"seq\": 2, \"kind\": \"claimed\", \"item\": \"x\", \"holder\": \"a\", \"at\": \"2031-05-04T12:00:01+00:00\", \"epoch\": 1, \"previous_holder\": null},",
        "{\"seq\": 3, \"kind\": \"taken_over\", \"item\": \"x\", \"holder\": \"b\", \"at\": \"2031-05-04T12:01:00+00:00\", \"epoch\": 2, \"previous_holder\": \"a\"},",
        "{\"seq\": 4, \"kind\": \"reset\", \"item\": \"x\", \"holder\": null, \"at\": \"2031-05-04T12:02:00+00:00\", \"epoch\": 2, \"previous_holder\": \"b\"}",
        "]}"
    );
    let err_v1_reset = assert_corrupt_and_unchanged(&check_path, v1_with_reset);
    assert!(err_v1_reset
        .message()
        .contains("holds an unknown kind 'reset'"));

    // 5b. `offered -> claimed -> taken_over(at cap=1) -> released -> claimed` (missing `reset`!) MUST fail!
    let claimed_when_exhausted = serde_json::json!({
        "format": "dibs-registry/2",
        "compaction": null,
        "items": [{
            "id": "x",
            "note": "",
            "status": "claimed",
            "priority": 2,
            "tags": [],
            "depends_on": [],
            "max_takeovers": 1,
            "takeover_count": 1,
            "holder": "c",
            "token_hash": "a".repeat(64),
            "epoch": 3,
            "expires_at": "2031-05-04T13:00:00+00:00"
        }],
        "history": [
            {"seq": 1, "kind": "offered", "item": "x", "holder": null, "at": "2031-05-04T12:00:00+00:00", "epoch": 0, "previous_holder": null},
            {"seq": 2, "kind": "claimed", "item": "x", "holder": "a", "at": "2031-05-04T12:00:01+00:00", "epoch": 1, "previous_holder": null},
            {"seq": 3, "kind": "taken_over", "item": "x", "holder": "b", "at": "2031-05-04T12:01:00+00:00", "epoch": 2, "previous_holder": "a"},
            {"seq": 4, "kind": "released", "item": "x", "holder": "b", "at": "2031-05-04T12:01:10+00:00", "epoch": 2, "previous_holder": null},
            {"seq": 5, "kind": "claimed", "item": "x", "holder": "c", "at": "2031-05-04T12:01:20+00:00", "epoch": 3, "previous_holder": null}
        ]
    });
    let err_exh_claim = assert_corrupt_and_unchanged(
        &check_path,
        &serde_json::to_string(&claimed_when_exhausted).unwrap(),
    );
    assert!(
        err_exh_claim
            .message()
            .contains("a claimed event on item 'x' exceeds max_takeovers 1"),
        "{}",
        err_exh_claim.message()
    );

    // 5c. Mid-life `reset` below minimum epoch `cap + 1`, mid-life `claimed -> released -> reset`,
    // mid-life consecutive `reset -> reset`, and mid-life `reset` ending with nonzero `takeover_count`!
    let mid_reset_low_epoch = serde_json::json!({
        "format": "dibs-registry/2",
        "compaction": null,
        "items": [{
            "id": "x",
            "note": "",
            "status": "open",
            "priority": 2,
            "tags": [],
            "depends_on": [],
            "max_takeovers": 2,
            "takeover_count": 0,
            "holder": null,
            "token_hash": null,
            "epoch": 2,
            "expires_at": null
        }],
        "history": [
            {"seq": 1, "kind": "reset", "item": "x", "holder": null, "at": "2031-05-04T12:02:00+00:00", "epoch": 2, "previous_holder": "b"}
        ]
    });
    let err_low_ep = assert_corrupt_and_unchanged(
        &check_path,
        &serde_json::to_string(&mid_reset_low_epoch).unwrap(),
    );
    assert!(
        err_low_ep
            .message()
            .contains("below the minimum epoch 3 for max_takeovers 2"),
        "{}",
        err_low_ep.message()
    );

    let mid_claim_rel_reset = serde_json::json!({
        "format": "dibs-registry/2",
        "compaction": null,
        "items": [{
            "id": "x",
            "note": "",
            "status": "open",
            "priority": 2,
            "tags": [],
            "depends_on": [],
            "max_takeovers": 2,
            "takeover_count": 0,
            "holder": null,
            "token_hash": null,
            "epoch": 5,
            "expires_at": null
        }],
        "history": [
            {"seq": 1, "kind": "claimed", "item": "x", "holder": "a", "at": "2031-05-04T12:00:00+00:00", "epoch": 5, "previous_holder": null},
            {"seq": 2, "kind": "released", "item": "x", "holder": "a", "at": "2031-05-04T12:00:10+00:00", "epoch": 5, "previous_holder": null},
            {"seq": 3, "kind": "reset", "item": "x", "holder": null, "at": "2031-05-04T12:00:20+00:00", "epoch": 5, "previous_holder": null}
        ]
    });
    let err_crr = assert_corrupt_and_unchanged(
        &check_path,
        &serde_json::to_string(&mid_claim_rel_reset).unwrap(),
    );
    assert!(
        err_crr
            .message()
            .contains("fires at takeover_count 1, below max_takeovers 2"),
        "{}",
        err_crr.message()
    );

    let mid_double_reset = serde_json::json!({
        "format": "dibs-registry/2",
        "compaction": null,
        "items": [{
            "id": "x",
            "note": "",
            "status": "open",
            "priority": 2,
            "tags": [],
            "depends_on": [],
            "max_takeovers": 1,
            "takeover_count": 0,
            "holder": null,
            "token_hash": null,
            "epoch": 4,
            "expires_at": null
        }],
        "history": [
            {"seq": 1, "kind": "reset", "item": "x", "holder": null, "at": "2031-05-04T12:00:00+00:00", "epoch": 4, "previous_holder": "a"},
            {"seq": 2, "kind": "reset", "item": "x", "holder": null, "at": "2031-05-04T12:00:10+00:00", "epoch": 4, "previous_holder": null}
        ]
    });
    let err_dbl = assert_corrupt_and_unchanged(
        &check_path,
        &serde_json::to_string(&mid_double_reset).unwrap(),
    );
    assert!(
        err_dbl
            .message()
            .contains("fires at takeover_count 0, below max_takeovers 1"),
        "{}",
        err_dbl.message()
    );

    let mid_reset_nonzero_count = serde_json::json!({
        "format": "dibs-registry/2",
        "compaction": null,
        "items": [{
            "id": "x",
            "note": "",
            "status": "open",
            "priority": 2,
            "tags": [],
            "depends_on": [],
            "max_takeovers": 1,
            "takeover_count": 1,
            "holder": null,
            "token_hash": null,
            "epoch": 4,
            "expires_at": null
        }],
        "history": [
            {"seq": 1, "kind": "reset", "item": "x", "holder": null, "at": "2031-05-04T12:00:00+00:00", "epoch": 4, "previous_holder": "a"}
        ]
    });
    let err_nz = assert_corrupt_and_unchanged(
        &check_path,
        &serde_json::to_string(&mid_reset_nonzero_count).unwrap(),
    );
    assert!(
        err_nz.message().contains(
            "item 'x' stands at takeover_count 1 where its history ends at takeover_count 0"
        ),
        "{}",
        err_nz.message()
    );

    // 6. Out-of-order dependency claim in history (`child` claimed at seq 3 before `gate` completes at seq 5)!
    let claim_before_dep_completed = serde_json::json!({
        "format": "dibs-registry/2",
        "compaction": null,
        "items": [
            {
                "id": "gate",
                "note": "",
                "status": "completed",
                "priority": 2,
                "tags": [],
                "depends_on": [],
                "max_takeovers": null,
                "takeover_count": 0,
                "holder": null,
                "token_hash": null,
                "epoch": 1,
                "expires_at": null
            },
            {
                "id": "child",
                "note": "",
                "status": "claimed",
                "priority": 2,
                "tags": [],
                "depends_on": ["gate"],
                "max_takeovers": null,
                "takeover_count": 0,
                "holder": "bob",
                "token_hash": "a".repeat(64),
                "epoch": 1,
                "expires_at": "2031-05-04T13:00:00+00:00"
            }
        ],
        "history": [
            {"seq": 1, "kind": "offered", "item": "gate", "holder": null, "at": "2031-05-04T12:00:00+00:00", "epoch": 0, "previous_holder": null},
            {"seq": 2, "kind": "offered", "item": "child", "holder": null, "at": "2031-05-04T12:00:01+00:00", "epoch": 0, "previous_holder": null},
            {"seq": 3, "kind": "claimed", "item": "child", "holder": "bob", "at": "2031-05-04T12:00:02+00:00", "epoch": 1, "previous_holder": null},
            {"seq": 4, "kind": "claimed", "item": "gate", "holder": "alice", "at": "2031-05-04T12:00:03+00:00", "epoch": 1, "previous_holder": null},
            {"seq": 5, "kind": "completed", "item": "gate", "holder": "alice", "at": "2031-05-04T12:00:04+00:00", "epoch": 1, "previous_holder": null}
        ]
    });
    let err_dep_ord = assert_corrupt_and_unchanged(
        &check_path,
        &serde_json::to_string(&claim_before_dep_completed).unwrap(),
    );
    assert!(
        err_dep_ord.message().contains(
            "the history of item 'child' claims while dependency 'gate' is not completed"
        ),
        "{}",
        err_dep_ord.message()
    );
}

#[test]
fn dag_depends_on_edges_diamond_readiness_and_deep_chain_no_stack_overflow() {
    let dir = temp_dir("dag-diamond-and-deep");
    let path = dir.join("registry.json");
    let lock_path = dir.join("registry.json.lock");
    let reg = Registry::new(path.clone());

    // 1. Self-dependency refused at InvalidRequest (exit 2) without touching disk.
    let self_err = expect_error(reg.offer_with(
        &OfferSpec::new("self-loop", "").with_depends_on(vec!["self-loop".to_string()]),
        &t0(),
    ));
    assert!(matches!(self_err, Error::InvalidRequest(_)), "{self_err:?}");
    assert_eq!(self_err.exit_code(), 2);
    assert!(!path.exists());
    assert!(!lock_path.exists());

    // 2. 65 distinct dependencies refused at InvalidRequest without touching disk.
    let sixty_five: Vec<String> = (0..65).map(|i| format!("dep-{i:02}")).collect();
    let over_err = expect_error(reg.offer_with(
        &OfferSpec::new("too-many-deps", "").with_depends_on(sixty_five),
        &t0(),
    ));
    assert!(matches!(over_err, Error::InvalidRequest(_)), "{over_err:?}");
    assert!(!path.exists());

    // 3. Unknown dependency refused at Refusal (exit 1), all-or-nothing in offer_many.
    let unk_err = expect_error(reg.offer_many(
        &[
            OfferSpec::new("good-one", ""),
            OfferSpec::new("bad-two", "").with_depends_on(vec!["missing-prereq".to_string()]),
        ],
        &t0(),
    ));
    assert!(matches!(unk_err, Error::Refusal(_)), "{unk_err:?}");
    assert_eq!(unk_err.exit_code(), 1);
    assert!(!path.exists());

    // 4. Intra-batch 2-node and 3-node cycles refused all-or-nothing at Refusal (exit 1).
    let cyc2_err = expect_error(reg.offer_many(
        &[
            OfferSpec::new("two-a", "").with_depends_on(vec!["two-b".to_string()]),
            OfferSpec::new("two-b", "").with_depends_on(vec!["two-a".to_string()]),
        ],
        &t0(),
    ));
    assert!(matches!(cyc2_err, Error::Refusal(_)), "{cyc2_err:?}");
    assert_eq!(cyc2_err.message(), "item dependencies contain a cycle");
    assert!(!path.exists());

    let cyc_err = expect_error(reg.offer_many(
        &[
            OfferSpec::new("cyc-a", "").with_depends_on(vec!["cyc-c".to_string()]),
            OfferSpec::new("cyc-b", "").with_depends_on(vec!["cyc-a".to_string()]),
            OfferSpec::new("cyc-c", "").with_depends_on(vec!["cyc-b".to_string()]),
        ],
        &t0(),
    ));
    assert!(matches!(cyc_err, Error::Refusal(_)), "{cyc_err:?}");
    assert_eq!(cyc_err.message(), "item dependencies contain a cycle");
    assert!(!path.exists());

    // 5. Diamond DAG offered with FORWARD intra-batch references (`d` listed first!).
    let offered = reg
        .offer_many(
            &[
                OfferSpec::new("step-d", "join")
                    .with_depends_on(vec!["step-c".to_string(), "step-b".to_string()]),
                OfferSpec::new("step-b", "left").with_depends_on(vec!["step-a".to_string()]),
                OfferSpec::new("step-c", "right").with_depends_on(vec!["step-a".to_string()]),
                OfferSpec::new("step-a", "root"),
            ],
            &t0(),
        )
        .unwrap();
    assert_eq!(offered, vec!["step-d", "step-b", "step-c", "step-a"]);

    // Verify depends_on of step-d was sorted & deduplicated on disk.
    let d_detail = reg.show("step-d", &t0()).unwrap();
    assert_eq!(d_detail.depends_on, vec!["step-b", "step-c"]);
    assert_eq!(d_detail.status, "open");
    assert_eq!(d_detail.effective_status, "blocked");

    // Ready returns only step-a, without creating a lock file!
    std::fs::remove_file(&lock_path).unwrap();
    let r0 = reg.ready(&[], &t0()).unwrap();
    assert!(!lock_path.exists());
    assert_eq!(
        r0.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
        vec!["step-a"]
    );

    // Board preserves offer order and shows blocked status on step-d, step-b, step-c.
    let b0 = reg.board(&t0()).unwrap();
    let statuses: Vec<(&str, &str)> = b0
        .iter()
        .map(|r| (r.id.as_str(), r.status.as_str()))
        .collect();
    assert_eq!(
        statuses,
        vec![
            ("step-d", "blocked"),
            ("step-b", "blocked"),
            ("step-c", "blocked"),
            ("step-a", "open"),
        ]
    );

    // Direct claim on a blocked item refuses with the exact DB-22 message.
    let blocked_claim = expect_error(reg.claim("step-b", "alice", 60, &at(1)));
    assert!(matches!(blocked_claim, Error::Refusal(_)));
    assert_eq!(
        blocked_claim.message(),
        "item 'step-b' is blocked by incomplete dependencies"
    );

    // Claiming step-a leaves step-b/c/d blocked until step-a is completed.
    let ga = reg.claim("step-a", "alice", 60, &at(2)).unwrap();
    assert!(reg.ready(&[], &at(3)).unwrap().is_empty());
    reg.complete("step-a", "alice", &ga.token, 1, &at(4))
        .unwrap();

    // Now step-b and step-c are ready (in offer order since both have priority 2), while step-d stays blocked!
    let r1 = reg.ready(&[], &at(5)).unwrap();
    assert_eq!(
        r1.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
        vec!["step-b", "step-c"]
    );
    assert_eq!(
        reg.show("step-d", &at(5)).unwrap().effective_status,
        "blocked"
    );

    // Completing step-b alone STILL leaves step-d blocked!
    let gb = reg.claim("step-b", "bob", 60, &at(6)).unwrap();
    reg.complete("step-b", "bob", &gb.token, 1, &at(7)).unwrap();
    assert_eq!(
        reg.show("step-d", &at(8)).unwrap().effective_status,
        "blocked"
    );
    let blocked_d = expect_error(reg.claim("step-d", "carol", 60, &at(8)));
    assert_eq!(
        blocked_d.message(),
        "item 'step-d' is blocked by incomplete dependencies"
    );

    // Completing step-c unblocks step-d!
    let gc = reg.claim("step-c", "carol", 60, &at(9)).unwrap();
    reg.complete("step-c", "carol", &gc.token, 1, &at(10))
        .unwrap();
    let r2 = reg.ready(&[], &at(11)).unwrap();
    assert_eq!(
        r2.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
        vec!["step-d"]
    );
    let gd = reg.claim("step-d", "dave", 60, &at(12)).unwrap();
    reg.complete("step-d", "dave", &gd.token, 1, &at(13))
        .unwrap();
    assert!(reg.board(&at(14)).unwrap().is_empty());

    // 6. 64 distinct dependencies accepted (with duplicates deduplicated down to 64).
    let dir64 = temp_dir("dag-64-deps");
    let reg64 = Registry::new(dir64.join("registry.json"));
    let prereq_specs: Vec<OfferSpec> = (0..64)
        .map(|i| OfferSpec::new(format!("pre-{i:02}"), ""))
        .collect();
    reg64.offer_many(&prereq_specs, &t0()).unwrap();
    let mut deps_with_dups: Vec<String> = (0..64).rev().map(|i| format!("pre-{i:02}")).collect();
    deps_with_dups.push("pre-00".to_string());
    deps_with_dups.push("pre-63".to_string());
    reg64
        .offer_with(
            &OfferSpec::new("fan-in-64", "").with_depends_on(deps_with_dups),
            &at(1),
        )
        .unwrap();
    assert_eq!(
        reg64.show("fan-in-64", &at(2)).unwrap().depends_on.len(),
        64
    );

    // 7. Deep 5,000-node dependency chain (`node-i` -> `node-(i+1)`) inside a tiny 256 KiB OS
    // thread stack pushes all 5,000 active DFS frames simultaneously from `start = 0`, proving
    // iterative heap-stack DFS never overflows the call stack on either cyclic or acyclic chains!
    let deep_dir = temp_dir("dag-deep-heap-dfs");
    let handle = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let deep_reg = Registry::new(deep_dir.join("registry.json"));
            let count = 5_000usize;
            let mut cyc_specs = Vec::with_capacity(count);
            for i in 0..count {
                let next = (i + 1) % count;
                cyc_specs.push(
                    OfferSpec::new(format!("node-{i:04}"), "")
                        .with_depends_on(vec![format!("node-{next:04}")]),
                );
            }
            let deep_cyc = expect_error(deep_reg.offer_many(&cyc_specs, &t0()));
            assert_eq!(deep_cyc.message(), "item dependencies contain a cycle");

            let mut specs = Vec::with_capacity(count);
            for i in 0..count {
                let mut sp = OfferSpec::new(format!("node-{i:04}"), "");
                if i + 1 < count {
                    sp = sp.with_depends_on(vec![format!("node-{:04}", i + 1)]);
                }
                specs.push(sp);
            }
            deep_reg.offer_many(&specs, &t0()).unwrap();
            let ready = deep_reg.ready(&[], &at(1)).unwrap();
            assert_eq!(ready.len(), 1);
            assert_eq!(ready[0].id, "node-4999");
        })
        .unwrap();
    handle.join().unwrap();

    // 8. Unix read-only directory check for `ready`: succeeds without creating a lock file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let ro_dir = temp_dir("ready-readonly-dir");
        let ro_path = ro_dir.join("registry.json");
        let ro_lock = ro_dir.join("registry.json.lock");
        let ro_reg = Registry::new(ro_path);
        ro_reg
            .offer_with(
                &OfferSpec::new("ro-item", "").with_tags(vec!["fast".to_string()]),
                &t0(),
            )
            .unwrap();
        std::fs::remove_file(&ro_lock).unwrap();
        std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let res = ro_reg.ready(&["fast".to_string()], &at(1));
        std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let list = res.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "ro-item");
        assert!(!ro_lock.exists());
    }
}

#[test]
fn priority_tags_ordering_and_and_filtering_in_ready_and_claim_next() {
    let dir = temp_dir("prio-and-tags");
    let path = dir.join("registry.json");
    let lock_path = dir.join("registry.json.lock");
    let reg = Registry::new(path.clone());

    // Invalid priority (> 3) and > 16 distinct tags and invalid tag names refuse at exit 2 without touching disk.
    let bad_prio = expect_error(reg.offer_with(&OfferSpec::new("bad", "").with_priority(4), &t0()));
    assert!(matches!(bad_prio, Error::InvalidRequest(_)));
    assert_eq!(bad_prio.exit_code(), 2);
    assert!(!path.exists());

    let seventeen_tags: Vec<String> = (0..17).map(|i| format!("t-{i:02}")).collect();
    let bad_tags =
        expect_error(reg.offer_with(&OfferSpec::new("bad", "").with_tags(seventeen_tags), &t0()));
    assert!(matches!(bad_tags, Error::InvalidRequest(_)));
    assert!(!path.exists());

    let bad_tag_name = expect_error(reg.offer_with(
        &OfferSpec::new("bad", "").with_tags(vec!["UPPER".to_string()]),
        &t0(),
    ));
    assert!(matches!(bad_tag_name, Error::InvalidName(_)));
    assert!(!path.exists());

    // Invalid tag on ready and claim_next_with_tags refuses at InvalidName without creating .lock.
    let bad_ready_tag = expect_error(reg.ready(&["bad tag".to_string()], &t0()));
    assert!(matches!(bad_ready_tag, Error::InvalidName(_)));
    assert!(!lock_path.exists());

    let bad_claim_tag =
        expect_error(reg.claim_next_with_tags("alice", 60, &["bad tag".to_string()], &t0()));
    assert!(matches!(bad_claim_tag, Error::InvalidName(_)));
    assert!(!lock_path.exists());

    // Validation-order symmetry on claim_next_with_tags & CLI claim-next:
    // holder is checked before ttl, and ttl is checked before tags!
    let order_holder_first =
        expect_error(reg.claim_next_with_tags("BAD HOLDER", 0, &["bad tag".to_string()], &t0()));
    assert!(order_holder_first
        .message()
        .starts_with("invalid holder id 'BAD HOLDER'"));
    let order_ttl_second =
        expect_error(reg.claim_next_with_tags("alice", 0, &["bad tag".to_string()], &t0()));
    assert!(matches!(order_ttl_second, Error::InvalidTtl(_)));

    let p_str = path.to_str().unwrap();
    let cli_order_holder = run_cli(
        &[
            "--registry",
            p_str,
            "claim-next",
            "--holder",
            "BAD HOLDER",
            "--ttl",
            "0",
            "--tag",
            "bad tag",
        ],
        None,
        2,
    );
    assert!(cli_order_holder
        .stderr
        .contains("invalid holder id 'BAD HOLDER'"));
    let cli_order_ttl = run_cli(
        &[
            "--registry",
            p_str,
            "claim-next",
            "--holder",
            "alice",
            "--ttl",
            "0",
            "--tag",
            "bad tag",
        ],
        None,
        2,
    );
    assert!(cli_order_ttl.stderr.contains("invalid time to live 0"));

    // 16 distinct tags accepted and normalized (sorted + deduplicated).
    let mut sixteen_tags: Vec<String> = (0..16).rev().map(|i| format!("cap-{i:02}")).collect();
    sixteen_tags.push("cap-00".to_string());
    reg.offer_with(
        &OfferSpec::new("p3-first", "")
            .with_priority(3)
            .with_tags(sixteen_tags),
        &t0(),
    )
    .unwrap();
    assert_eq!(reg.show("p3-first", &t0()).unwrap().tags.len(), 16);

    reg.offer_many(
        &[
            OfferSpec::new("p2-first", "")
                .with_priority(2)
                .with_tags(vec!["linux".to_string(), "gpu".to_string()]),
            OfferSpec::new("p0-first", "")
                .with_priority(0)
                .with_tags(vec!["linux".to_string()]),
            OfferSpec::new("p1-first", "")
                .with_priority(1)
                .with_tags(vec![
                    "gpu".to_string(),
                    "linux".to_string(),
                    "nvme".to_string(),
                ]),
            OfferSpec::new("p0-second", "")
                .with_priority(0)
                .with_tags(vec!["linux".to_string(), "gpu".to_string()]),
            OfferSpec::new("p2-second", "")
                .with_priority(2)
                .with_tags(vec!["macos".to_string()]),
        ],
        &at(1),
    )
    .unwrap();

    // Board preserves original offer order.
    let board_ids: Vec<String> = reg
        .board(&at(2))
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(
        board_ids,
        vec![
            "p3-first",
            "p2-first",
            "p0-first",
            "p1-first",
            "p0-second",
            "p2-second"
        ]
    );

    // Ready orders by (priority ASC, offer_order ASC).
    let all_ready: Vec<String> = reg
        .ready(&[], &at(2))
        .unwrap()
        .into_iter()
        .map(|d| d.id)
        .collect();
    assert_eq!(
        all_ready,
        vec![
            "p0-first",
            "p0-second",
            "p1-first",
            "p2-first",
            "p2-second",
            "p3-first"
        ]
    );

    // Multi-tag AND filtering in ready: must match BOTH "linux" and "gpu".
    let gpu_linux_ready: Vec<String> = reg
        .ready(&["gpu".to_string(), "linux".to_string()], &at(2))
        .unwrap()
        .into_iter()
        .map(|d| d.id)
        .collect();
    assert_eq!(gpu_linux_ready, vec!["p0-second", "p1-first", "p2-first"]);

    // claim_next_with_tags claims in exact (priority ASC, offer_order ASC) order among matching items!
    let c1 = reg
        .claim_next_with_tags("w1", 60, &["linux".to_string(), "gpu".to_string()], &at(3))
        .unwrap();
    assert_eq!(c1.item, "p0-second");
    let c2 = reg
        .claim_next_with_tags("w2", 60, &["linux".to_string(), "gpu".to_string()], &at(4))
        .unwrap();
    assert_eq!(c2.item, "p1-first");
    let c3 = reg
        .claim_next_with_tags("w3", 60, &["linux".to_string(), "gpu".to_string()], &at(5))
        .unwrap();
    assert_eq!(c3.item, "p2-first");

    // All linux+gpu items are now live-claimed -> next call refuses with exact message!
    let none_left = expect_error(reg.claim_next_with_tags(
        "w4",
        60,
        &["linux".to_string(), "gpu".to_string()],
        &at(6),
    ));
    assert!(matches!(none_left, Error::Refusal(_)));
    assert_eq!(none_left.message(), "no ready item stands in the backlog");

    // Unfiltered claim_next claims p0-first next!
    let c4 = reg.claim_next("w4", 60, &at(7)).unwrap();
    assert_eq!(c4.item, "p0-first");
}

#[test]
fn max_takeovers_exhausted_precedence_and_reset_lifecycle() {
    let dir = temp_dir("max-takeovers-reset");
    let path = dir.join("registry.json");
    let reg = Registry::new(path.clone());

    // Bounds check: 0 and 1001 refused at InvalidRequest (exit 2); 1 and 1000 accepted.
    for bad in [0u64, 1001] {
        let err = expect_error(reg.offer_with(
            &OfferSpec::new("bad-cap", "").with_max_takeovers(Some(bad)),
            &t0(),
        ));
        assert!(matches!(err, Error::InvalidRequest(_)));
        assert_eq!(err.exit_code(), 2);
    }
    assert!(!path.exists());

    reg.offer_with(
        &OfferSpec::new("cap-1000", "").with_max_takeovers(Some(1000)),
        &t0(),
    )
    .unwrap();
    assert_eq!(
        reg.show("cap-1000", &t0()).unwrap().max_takeovers,
        Some(1000)
    );

    // Lifecycle for `flaky` with max_takeovers = Some(2):
    reg.offer_with(
        &OfferSpec::new("flaky", "bounded retries")
            .with_priority(0)
            .with_max_takeovers(Some(2)),
        &at(1),
    )
    .unwrap();

    // Reset on open item refuses.
    let open_reset = expect_error(reg.reset("flaky", &at(2)));
    assert!(matches!(open_reset, Error::Refusal(_)));
    assert_eq!(
        open_reset.message(),
        "item 'flaky' is not exhausted and needs no reset"
    );

    // Reset on unknown item refuses with NotFound.
    let missing_reset = expect_error(reg.reset("no-such-item", &at(2)));
    assert!(matches!(missing_reset, Error::NotFound(_)));

    // Epoch 1 initial claim (takeover_count = 0), expires at at(12).
    let _g1 = reg.claim("flaky", "alice", 10, &at(2)).unwrap();
    // Epoch 2 first takeover at at(12) (takeover_count = 1 < 2), expires at at(22).
    let _g2 = reg.claim("flaky", "bob", 10, &at(12)).unwrap();
    assert_eq!(reg.show("flaky", &at(22)).unwrap().effective_status, "open");
    assert_eq!(reg.show("flaky", &at(22)).unwrap().takeover_count, 1);

    // Epoch 3 second takeover at at(22) (takeover_count = 2 == cap), expires at at(32).
    let g3 = reg.claim("flaky", "carol", 10, &at(22)).unwrap();
    assert_eq!(g3.epoch, 3);

    // CRITICAL EDGE: while carol's 2nd takeover lease is STILL LIVE (at(25) < at(32)),
    // effective_status is "claimed", carol can renew it, and reset is refused!
    let live_at_cap = reg.show("flaky", &at(25)).unwrap();
    assert_eq!(live_at_cap.status, "claimed");
    assert_eq!(live_at_cap.effective_status, "claimed");
    assert_eq!(live_at_cap.takeover_count, 2);
    let live_reset = expect_error(reg.reset("flaky", &at(25)));
    assert_eq!(
        live_reset.message(),
        "item 'flaky' holds a live lease by 'carol' and cannot be reset"
    );
    let new_exp = reg
        .renew("flaky", "carol", &g3.token, 3, 10, &at(25))
        .unwrap();
    assert_eq!(new_exp, at(42));

    // At exact expiry at(42): status becomes "open", effective_status becomes "exhausted"!
    let exh = reg.show("flaky", &at(42)).unwrap();
    assert_eq!(exh.status, "open");
    assert_eq!(exh.effective_status, "exhausted");
    assert_eq!(exh.holder, None);
    assert_eq!(exh.expires_at, None);
    assert_eq!(exh.epoch, 3);
    assert_eq!(exh.takeover_count, 2);

    // Board shows "exhausted", ready omits "flaky", claim_next skips "flaky" and claims "cap-1000"!
    let board_map: std::collections::HashMap<String, String> = reg
        .board(&at(42))
        .unwrap()
        .into_iter()
        .map(|r| (r.id, r.status))
        .collect();
    assert_eq!(
        board_map.get("flaky").map(String::as_str),
        Some("exhausted")
    );
    assert_eq!(
        reg.ready(&[], &at(42))
            .unwrap()
            .into_iter()
            .map(|d| d.id)
            .collect::<Vec<_>>(),
        vec!["cap-1000"]
    );
    let skipped = reg.claim_next("dave", 60, &at(42)).unwrap();
    assert_eq!(skipped.item, "cap-1000");

    // Direct claim on exhausted item refuses with exact DB-23 message.
    let exh_claim = expect_error(reg.claim("flaky", "dave", 60, &at(43)));
    assert!(matches!(exh_claim, Error::Refusal(_)));
    assert_eq!(
        exh_claim.message(),
        "item 'flaky' has reached max_takeovers and must be reset"
    );

    // Reset from expired-claimed exhausted state records previous_holder = Some("carol"),
    // clears takeover_count to 0, keeps epoch = 3, and sets status & effective_status = "open"!
    let reset_detail = reg.reset("flaky", &at(44)).unwrap();
    assert_eq!(reset_detail.status, "open");
    assert_eq!(reset_detail.effective_status, "open");
    assert_eq!(reset_detail.epoch, 3);
    assert_eq!(reset_detail.takeover_count, 0);
    assert_eq!(reset_detail.holder, None);

    let flaky_events = reg.history(Some("flaky")).unwrap();
    let last_ev = flaky_events.last().unwrap();
    assert_eq!(last_ev.kind, "reset");
    assert_eq!(last_ev.holder, None);
    assert_eq!(last_ev.previous_holder.as_deref(), Some("carol"));
    assert_eq!(last_ev.epoch, 3);

    // Next claim after reset is "claimed" (NOT "taken_over") at epoch 4 with takeover_count = 0!
    let g4 = reg.claim("flaky", "dave", 60, &at(45)).unwrap();
    assert_eq!(g4.epoch, 4);
    let flaky_events = reg.history(Some("flaky")).unwrap();
    assert_eq!(flaky_events.last().unwrap().kind, "claimed");
    assert_eq!(flaky_events.last().unwrap().previous_holder, None);
    reg.complete("flaky", "dave", &g4.token, 4, &at(46))
        .unwrap();

    // Reset on completed item refuses.
    let done_reset = expect_error(reg.reset("flaky", &at(47)));
    assert_eq!(
        done_reset.message(),
        "item 'flaky' is completed and cannot be reset"
    );

    // Case B: Reset when lease at max_takeovers was RELEASED before expiry (previous_holder == None)!
    reg.offer_with(
        &OfferSpec::new("released-cap", "").with_max_takeovers(Some(1)),
        &at(50),
    )
    .unwrap();
    let _r1 = reg.claim("released-cap", "alice", 10, &at(51)).unwrap();
    let r2 = reg.claim("released-cap", "bob", 60, &at(65)).unwrap();
    assert_eq!(r2.epoch, 2);
    // Bob releases while still live at takeover_count == 1 == max_takeovers:
    reg.release("released-cap", "bob", &r2.token, 2, &at(70))
        .unwrap();
    let rel_detail = reg.show("released-cap", &at(71)).unwrap();
    assert_eq!(rel_detail.status, "open");
    assert_eq!(rel_detail.effective_status, "exhausted");
    reg.reset("released-cap", &at(72)).unwrap();
    let rel_events = reg.history(Some("released-cap")).unwrap();
    assert_eq!(rel_events.last().unwrap().kind, "reset");
    assert_eq!(rel_events.last().unwrap().previous_holder, None);
    assert_eq!(rel_events.last().unwrap().epoch, 2);

    // Case B2: Completing a live lease when takeover_count == max_takeovers succeeds!
    reg.offer_with(
        &OfferSpec::new("live-cap-complete", "").with_max_takeovers(Some(1)),
        &at(75),
    )
    .unwrap();
    let _lc1 = reg
        .claim("live-cap-complete", "alice", 10, &at(76))
        .unwrap();
    let lc2 = reg.claim("live-cap-complete", "bob", 60, &at(90)).unwrap();
    assert_eq!(lc2.epoch, 2);
    assert_eq!(
        reg.show("live-cap-complete", &at(91))
            .unwrap()
            .takeover_count,
        1
    );
    reg.complete("live-cap-complete", "bob", &lc2.token, 2, &at(92))
        .unwrap();
    let lc_done = reg.show("live-cap-complete", &at(93)).unwrap();
    assert_eq!(lc_done.status, "completed");
    assert_eq!(lc_done.effective_status, "completed");

    // Case C: Precedence pin (`blocked` > `exhausted`).
    // Stage a valid /2 registry where `child` depends on open `gate` AND already holds
    // takeover_count == max_takeovers == 1. While `gate` is incomplete, `child` has
    // effective_status == "blocked", so `reset` refuses; once `gate` completes, `child`
    // reveals effective_status == "exhausted" and `reset` succeeds!
    let prec_dir = temp_dir("blocked-over-exhausted");
    let prec_path = prec_dir.join("registry.json");
    let prec_doc = serde_json::json!({
        "format": "dibs-registry/2",
        "compaction": null,
        "items": [
            {
                "id": "gate",
                "note": "",
                "status": "open",
                "holder": null,
                "token_hash": null,
                "epoch": 0,
                "expires_at": null,
                "priority": 2,
                "tags": [],
                "depends_on": [],
                "max_takeovers": null,
                "takeover_count": 0
            },
            {
                "id": "child",
                "note": "",
                "status": "open",
                "holder": null,
                "token_hash": null,
                "epoch": 2,
                "expires_at": null,
                "priority": 2,
                "tags": [],
                "depends_on": ["gate"],
                "max_takeovers": 1,
                "takeover_count": 1
            }
        ],
        "history": []
    });
    std::fs::write(&prec_path, serde_json::to_string(&prec_doc).unwrap()).unwrap();
    let prec_reg = Registry::new(prec_path);
    assert_eq!(
        prec_reg.show("child", &at(100)).unwrap().effective_status,
        "blocked"
    );
    let blocked_reset = expect_error(prec_reg.reset("child", &at(100)));
    assert_eq!(
        blocked_reset.message(),
        "item 'child' is blocked by incomplete dependencies and is not exhausted"
    );
    let g_gate = prec_reg.claim("gate", "alice", 60, &at(101)).unwrap();
    prec_reg
        .complete("gate", "alice", &g_gate.token, 1, &at(102))
        .unwrap();
    assert_eq!(
        prec_reg.show("child", &at(103)).unwrap().effective_status,
        "exhausted"
    );
    let unlocked_reset = prec_reg.reset("child", &at(104)).unwrap();
    assert_eq!(unlocked_reset.effective_status, "open");
    assert_eq!(unlocked_reset.takeover_count, 0);
}

#[test]
fn compact_crash_idempotence_incremental_archival_and_alias_refusals() {
    let dir = temp_dir("compact-idempotent");
    let path = dir.join("registry.json");
    let lock_path = dir.join("registry.json.lock");
    let archive = dir.join("events.jsonl");
    let reg = Registry::new(path.clone());

    // 1. through_seq == 0 refuses at InvalidRequest (exit 2) without touching disk.
    let zero_err = expect_error(reg.compact(0, &archive, &t0()));
    assert!(matches!(zero_err, Error::InvalidRequest(_)));
    assert_eq!(zero_err.exit_code(), 2);
    assert!(!path.exists());
    assert!(!archive.exists());

    // 2. Invalid archive paths refuse at InvalidRequest (exit 2).
    for bad_arch in ["", ".", "..", "sub/"] {
        let err = expect_error(reg.compact(1, Path::new(bad_arch), &t0()));
        assert!(
            matches!(err, Error::InvalidRequest(_)),
            "bad_arch={bad_arch:?}"
        );
        assert_eq!(err.exit_code(), 2);
    }

    // 3. Archive aliasing registry or registry.lock refuses at InvalidRequest (exit 2)
    // BEFORE creating registry or `.lock` on disk, including symlinked ancestor + `..`
    // and case-insensitive filename variants!
    let sub_dir = dir.join("nested");
    std::fs::create_dir_all(&sub_dir).unwrap();
    for alias in [
        path.clone(),
        lock_path.clone(),
        sub_dir.join("..").join("registry.json"),
        sub_dir.join("..").join("registry.json.lock"),
        dir.join("nonexistent").join("..").join("registry.json"),
        dir.join("nonexistent")
            .join("..")
            .join("registry.json.lock"),
    ] {
        let err = expect_error(reg.compact(1, &alias, &t0()));
        assert!(matches!(err, Error::InvalidRequest(_)), "alias={alias:?}");
        assert_eq!(err.exit_code(), 2);
    }
    #[cfg(unix)]
    {
        let deep_target = dir.join("real_a").join("real_b");
        std::fs::create_dir_all(&deep_target).unwrap();
        let sym_dir = dir.join("sym_dir");
        std::os::unix::fs::symlink(&deep_target, &sym_dir).unwrap();
        for sym_parent_alias in [
            sym_dir.join("..").join("..").join("registry.json"),
            sym_dir.join("..").join("..").join("registry.json.lock"),
        ] {
            let err = expect_error(reg.compact(1, &sym_parent_alias, &t0()));
            assert!(
                matches!(err, Error::InvalidRequest(_)),
                "sym_parent_alias={sym_parent_alias:?}, err={err:?}"
            );
            assert_eq!(err.exit_code(), 2);
            assert!(!path.exists());
            assert!(!lock_path.exists());
        }
    }
    #[cfg(any(target_os = "macos", windows))]
    {
        for case_alias in [dir.join("REGISTRY.JSON"), dir.join("REGISTRY.JSON.LOCK")] {
            let err = expect_error(reg.compact(1, &case_alias, &t0()));
            assert!(
                matches!(err, Error::InvalidRequest(_)),
                "case_alias={case_alias:?}, err={err:?}"
            );
            assert_eq!(err.exit_code(), 2);
            assert!(!path.exists());
            assert!(!lock_path.exists());
        }
    }

    // Also refuse when `<archive>.lock` would overwrite `<registry>`!
    let reg_named_lock = Registry::new(dir.join("events.jsonl.lock"));
    let err_arch_lock_alias = expect_error(reg_named_lock.compact(1, &archive, &t0()));
    assert!(matches!(err_arch_lock_alias, Error::InvalidRequest(_)));
    assert_eq!(err_arch_lock_alias.exit_code(), 2);

    // Missing registry refuses at Refusal (exit 1).
    let missing_err = expect_error(reg.compact(1, &archive, &t0()));
    assert!(matches!(missing_err, Error::Refusal(_)));
    assert_eq!(missing_err.exit_code(), 1);

    // Seed rich history of 9 events across 3 items (including takeover and reset):
    // seq 1: offer alpha (max_takeovers=1)
    // seq 2: offer beta
    // seq 3: claim alpha (alice, epoch 1)
    // seq 4: taken_over alpha (bob, epoch 2, takeover_count=1)
    // seq 5: reset alpha (epoch 2, takeover_count=0)
    // seq 6: claim alpha (carol, epoch 3)
    // seq 7: complete alpha (carol, epoch 3)
    // seq 8: claim beta (dave, epoch 1)
    // seq 9: renew beta (dave, epoch 1)
    reg.offer_with(
        &OfferSpec::new("alpha", "note a").with_max_takeovers(Some(1)),
        &t0(),
    )
    .unwrap();
    reg.offer("beta", "note b", &at(1)).unwrap();
    let _ga1 = reg.claim("alpha", "alice", 10, &at(2)).unwrap();
    let _ga2 = reg.claim("alpha", "bob", 10, &at(15)).unwrap();
    reg.reset("alpha", &at(30)).unwrap();
    let ga3 = reg.claim("alpha", "carol", 60, &at(31)).unwrap();
    reg.complete("alpha", "carol", &ga3.token, 3, &at(32))
        .unwrap();
    let gb1 = reg.claim("beta", "dave", 60, &at(33)).unwrap();
    reg.renew("beta", "dave", &gb1.token, 1, 60, &at(34))
        .unwrap();

    #[cfg(unix)]
    {
        // Also test symlink-parent + `..` and case-alias against `.lock` when registry
        // exists on disk and `.lock` has been removed (ensuring `.lock` is NOT created!).
        std::fs::remove_file(&lock_path).unwrap();
        let sym_lock_dotdot = dir
            .join("sym_dir")
            .join("..")
            .join("..")
            .join("registry.json.lock");
        let err_dd = expect_error(reg.compact(4, &sym_lock_dotdot, &at(40)));
        assert!(matches!(err_dd, Error::InvalidRequest(_)));
        assert!(!lock_path.exists());

        #[cfg(target_os = "macos")]
        {
            let err_case_lock =
                expect_error(reg.compact(4, &dir.join("REGISTRY.JSON.LOCK"), &at(40)));
            assert!(matches!(err_case_lock, Error::InvalidRequest(_)));
            assert!(!lock_path.exists());
        }

        // Restore `.lock` for symlink & hard-link alias checks against live registry and `.lock`!
        std::fs::write(&lock_path, b"").unwrap();
        let sym_reg = dir.join("sym-to-reg.jsonl");
        let sym_lock = dir.join("sym-to-lock.jsonl");
        let hard_reg = dir.join("hard-to-reg.jsonl");
        std::os::unix::fs::symlink(&path, &sym_reg).unwrap();
        std::os::unix::fs::symlink(&lock_path, &sym_lock).unwrap();
        std::fs::hard_link(&path, &hard_reg).unwrap();
        for link_alias in [&sym_reg, &sym_lock, &hard_reg] {
            let err = expect_error(reg.compact(4, link_alias, &at(40)));
            assert!(
                matches!(err, Error::InvalidRequest(_)),
                "link_alias={link_alias:?}, err={err:?}"
            );
            assert_eq!(err.exit_code(), 2);
        }
        std::fs::remove_file(&sym_reg).unwrap();
        std::fs::remove_file(&sym_lock).unwrap();
        std::fs::remove_file(&hard_reg).unwrap();
    }

    // through_seq > last_seq (9) refuses at exit 1.
    let past_end = expect_error(reg.compact(10, &archive, &at(40)));
    assert!(matches!(past_end, Error::Refusal(_)));
    assert_eq!(
        past_end.message(),
        "requested through_seq 10 exceeds the last recorded seq 9"
    );

    // Initial compaction through seq 4 (right when alpha had takeover_count=1 at bob's takeover!):
    let c1 = reg.compact(4, &archive, &at(40)).unwrap();
    assert_eq!(c1.through_seq, 4);
    assert_eq!(c1.archived_count, 4);
    assert_eq!(c1.remaining_count, 5);
    assert_eq!(c1.archive_path, archive);
    let arch1_bytes = std::fs::read(&archive).unwrap();
    assert_eq!(c1.archived_events_sha256, sha256_hex(&arch1_bytes));
    let arch1_lines: Vec<&str> = std::str::from_utf8(&arch1_bytes).unwrap().lines().collect();
    assert_eq!(arch1_lines.len(), 4);

    // Remaining history in registry holds seq 5..=9 and validates cleanly!
    let rem_hist = reg.history(None).unwrap();
    assert_eq!(
        rem_hist.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![5, 6, 7, 8, 9]
    );

    // Re-running with through_seq <= 4 refuses at exit 1.
    for stale_seq in [1u64, 4] {
        let err = expect_error(reg.compact(stale_seq, &archive, &at(41)));
        assert!(matches!(err, Error::Refusal(_)));
        assert_eq!(
            err.message(),
            "the registry is already compacted through seq 4"
        );
    }

    // Save snapshot at through_seq == 4 to test both mid-compact crash scenarios!
    let reg_at_4 = std::fs::read(&path).unwrap();

    // Perform clean reference compact through 7 on a copy to capture canonical lines 5..=7:
    let ref_archive = dir.join("ref.jsonl");
    std::fs::write(&ref_archive, &arch1_bytes).unwrap();
    let ref_summary = reg.compact(7, &ref_archive, &at(45)).unwrap();
    let ref_bytes = std::fs::read(&ref_archive).unwrap();
    let ref_lines: Vec<&str> = std::str::from_utf8(&ref_bytes).unwrap().lines().collect();
    assert_eq!(ref_lines.len(), 7);

    // CRASH CASE A: Archive received lines 5 and 6 (1..=6 on disk), then process crashed
    // BEFORE line 7 or registry rename! Retry `compact(7, &archive, ...)` must verify 1..=4,
    // match 5..=6 without duplicating them, and append ONLY line 7!
    std::fs::write(&path, &reg_at_4).unwrap();
    let mut partial_1_to_6 = String::new();
    for line in &ref_lines[..6] {
        partial_1_to_6.push_str(line);
        partial_1_to_6.push('\n');
    }
    std::fs::write(&archive, partial_1_to_6.as_bytes()).unwrap();

    let retry_a = reg.compact(7, &archive, &at(46)).unwrap();
    assert_eq!(retry_a.through_seq, 7);
    assert_eq!(retry_a.archived_count, 3);
    assert_eq!(retry_a.remaining_count, 2);
    assert_eq!(
        retry_a.archived_events_sha256,
        ref_summary.archived_events_sha256
    );
    assert_eq!(std::fs::read(&archive).unwrap(), ref_bytes);

    // CRASH CASE B: Archive received ALL lines 1..=7 and synced, then crashed right before
    // `fs::rename(tmp, registry)`! Retry `compact(7, &archive, ...)` appends 0 bytes and
    // completes the registry publish!
    std::fs::write(&path, &reg_at_4).unwrap();
    assert_eq!(std::fs::read(&archive).unwrap(), ref_bytes);
    let retry_b = reg.compact(7, &archive, &at(47)).unwrap();
    assert_eq!(retry_b.through_seq, 7);
    assert_eq!(
        retry_b.archived_events_sha256,
        ref_summary.archived_events_sha256
    );
    assert_eq!(std::fs::read(&archive).unwrap(), ref_bytes);

    // CRASH CASE C (Torn final record recovery): Archive wrote complete lines 1..=5 and
    // crashed mid-write 19 bytes into line 6 (no trailing `\n`). Retry `compact(7, &archive, ...)`
    // completes line 6, appends line 7, and produces the exact 7-line archive with zero duplicates!
    std::fs::write(&path, &reg_at_4).unwrap();
    let mut torn_1_to_6 = Vec::new();
    for line in &ref_lines[..5] {
        torn_1_to_6.extend_from_slice(line.as_bytes());
        torn_1_to_6.push(b'\n');
    }
    torn_1_to_6.extend_from_slice(&ref_lines[5].as_bytes()[..19]);
    std::fs::write(&archive, &torn_1_to_6).unwrap();
    let retry_c = reg.compact(7, &archive, &at(47)).unwrap();
    assert_eq!(retry_c.through_seq, 7);
    assert_eq!(
        retry_c.archived_events_sha256,
        ref_summary.archived_events_sha256
    );
    assert_eq!(std::fs::read(&archive).unwrap(), ref_bytes);

    // Corrupt / mismatched archive refusals on incremental compact(9, &archive, ...) leave BOTH files untouched!
    let reg_at_7 = std::fs::read(&path).unwrap();
    let bad_archives: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "missing-trailing-nl",
            b"{\"seq\": 1}".to_vec(),
            "fewer than",
        ),
        (
            "garbage-torn-tail",
            {
                let mut out = ref_bytes.clone();
                out.extend_from_slice(b"{\"seq\": 8, \"kind\": \"BOGUS");
                out
            },
            "not canonical JSON Lines",
        ),
        ("invalid-json", b"not-json\n".to_vec(), "is not valid JSON"),
        (
            "duplicate-key",
            format!(
                "{}\n",
                ref_lines[0].replacen("\"seq\": 1", "\"seq\": 1, \"seq\": 1", 1)
            )
            .into_bytes(),
            "carries duplicate key seq",
        ),
        (
            "non-canonical-whitespace",
            {
                let mut out = String::from_utf8(ref_bytes.clone()).unwrap();
                let ev8 = &reg.history(None).unwrap()[0];
                out.push_str(&format!(
                    "{{  \"seq\": {}, \"kind\": \"{}\", \"item\": \"{}\", \"holder\": \"{}\", \"at\": \"{}\", \"epoch\": {}, \"previous_holder\": null}}\n",
                    ev8.seq,
                    ev8.kind,
                    ev8.item,
                    ev8.holder.as_deref().unwrap(),
                    ev8.at,
                    ev8.epoch
                ));
                out.into_bytes()
            },
            "archive line 8 is not in canonical JSON Lines form",
        ),
        (
            "truncated-prefix",
            format!("{}\n{}\n", ref_lines[0], ref_lines[1]).into_bytes(),
            "fewer than the standing compaction",
        ),
        (
            "tampered-prefix-digest",
            {
                let mut lines: Vec<String> = ref_lines.iter().map(|s| (*s).to_string()).collect();
                lines[1] =
                    lines[1].replace("2031-05-04T12:00:01+00:00", "2031-05-04T12:00:09+00:00");
                let mut joined = lines.join("\n");
                joined.push('\n');
                joined.into_bytes()
            },
            "digest does not match",
        ),
        (
            "mismatched-pending-event",
            {
                let mut out = String::from_utf8(ref_bytes.clone()).unwrap();
                // Append seq 8 claiming a wrong holder ("eve" instead of "dave")!
                out.push_str("{\"seq\": 8, \"kind\": \"claimed\", \"item\": \"beta\", \"holder\": \"eve\", \"at\": \"2031-05-04T12:00:33+00:00\", \"epoch\": 1, \"previous_holder\": null}\n");
                out.into_bytes()
            },
            "archive event at seq 8 disagrees",
        ),
        (
            "archive-beyond-requested-seq",
            {
                let mut out = String::from_utf8(ref_bytes.clone()).unwrap();
                let rem = reg.history(None).unwrap();
                for ev in &rem {
                    out.push_str(&format!(
                        "{{\"seq\": {}, \"kind\": \"{}\", \"item\": \"{}\", \"holder\": \"{}\", \"at\": \"{}\", \"epoch\": {}, \"previous_holder\": null}}\n",
                        ev.seq,
                        ev.kind,
                        ev.item,
                        ev.holder.as_deref().unwrap(),
                        ev.at,
                        ev.epoch
                    ));
                }
                out.into_bytes()
            },
            "beyond requested through_seq 8",
        ),
    ];

    for (label, bad_bytes, expected_msg) in bad_archives {
        // For "archive-beyond-requested-seq", archive has 9 events and we request compact(8):
        let target_seq = if label == "archive-beyond-requested-seq" {
            8
        } else {
            9
        };
        std::fs::write(&archive, &bad_bytes).unwrap();
        let err = expect_error(reg.compact(target_seq, &archive, &at(48)));
        assert!(
            matches!(err, Error::Refusal(_)),
            "case {label} returned {err:?}"
        );
        assert!(
            err.message().contains(expected_msg),
            "case {label}: expected {expected_msg:?} in {:?}",
            err.message()
        );
        assert_eq!(std::fs::read(&archive).unwrap(), bad_bytes, "case {label}");
        assert_eq!(std::fs::read(&path).unwrap(), reg_at_7, "case {label}");
    }

    // Exclusive `<archive>.lock` contention pin: holding an exclusive lock on `events.jsonl.lock`
    // blocks `compact` until the lock is released!
    std::fs::write(&archive, &ref_bytes).unwrap();
    let archive_lock = dir.join("events.jsonl.lock");
    let mut arch_blocker_opts = std::fs::OpenOptions::new();
    arch_blocker_opts
        .read(true)
        .write(true)
        .create(true)
        .truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        arch_blocker_opts.share_mode(0x0000_0001 | 0x0000_0002);
    }
    let arch_blocker = arch_blocker_opts.open(&archive_lock).unwrap();
    arch_blocker.lock().unwrap();

    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let finished_bg = std::sync::Arc::clone(&finished);
    let path_bg = path.clone();
    let arch_bg = archive.clone();
    let compact_thread = std::thread::spawn(move || {
        let res = Registry::new(path_bg).compact(9, &arch_bg, &at(50));
        finished_bg.store(true, std::sync::atomic::Ordering::SeqCst);
        res
    });
    std::thread::sleep(std::time::Duration::from_millis(40));
    assert!(
        !finished.load(std::sync::atomic::Ordering::SeqCst),
        "compact must wait on <archive>.lock"
    );
    arch_blocker.unlock().unwrap();
    drop(arch_blocker);

    let c9 = compact_thread.join().unwrap().unwrap();
    assert_eq!(c9.through_seq, 9);
    assert_eq!(c9.archived_count, 2);
    assert_eq!(c9.remaining_count, 0);
    assert!(reg.history(None).unwrap().is_empty());

    reg.offer("gamma", "post-compact item", &at(51)).unwrap();
    let gg = reg.claim("gamma", "zoe", 60, &at(52)).unwrap();
    reg.complete("gamma", "zoe", &gg.token, 1, &at(53)).unwrap();
    let post_events = reg.history(None).unwrap();
    assert_eq!(
        post_events.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![10, 11, 12]
    );
}

#[test]
fn cli_v2_end_to_end_human_and_json_modes() {
    let dir = temp_dir("cli-v2-end-to-end");
    let path = dir.join("registry.json");
    let archive = dir.join("archive.jsonl");
    let p = path.to_str().unwrap();
    let a = archive.to_str().unwrap();

    // 1. CLI offer with --priority, repeatable --tag, --max-takeovers
    let off1 = run_cli(
        &[
            "--registry",
            p,
            "offer",
            "build-core",
            "--priority",
            "0",
            "--tag",
            "linux",
            "--tag",
            "rust",
            "--max-takeovers",
            "1",
            "--json",
        ],
        None,
        0,
    );
    assert_eq!(
        off1.stdout,
        "{\"item\": \"build-core\", \"status\": \"open\", \"epoch\": 0}\n"
    );

    // 2. CLI offer-batch with /2 JSONL fields and intra-batch forward dependency
    let batch_jsonl = concat!(
        "{\"id\": \"deploy-app\", \"priority\": 1, \"tags\": [\"prod\", \"linux\"], \"depends_on\": [\"test-suite\"]}\n",
        "{\"id\": \"test-suite\", \"priority\": 1, \"tags\": [\"linux\"], \"depends_on\": [\"build-core\"], \"max_takeovers\": null}\n"
    );
    let batch = run_cli(
        &["--registry", p, "offer-batch", "--json"],
        Some(batch_jsonl.as_bytes()),
        0,
    );
    assert_eq!(
        batch.stdout,
        "{\"count\": 2, \"offered\": [\"deploy-app\", \"test-suite\"]}\n"
    );

    // 2b. CLI offer with repeatable --depends-on and --tag, plus full 12-line human `show` check!
    let off_dep = run_cli(
        &[
            "--registry",
            p,
            "offer",
            "package-bin",
            "--note",
            "ship artifact",
            "--priority",
            "2",
            "--tag",
            "release",
            "--tag",
            "linux",
            "--depends-on",
            "deploy-app",
            "--depends-on",
            "build-core",
            "--max-takeovers",
            "3",
        ],
        None,
        0,
    );
    assert_eq!(off_dep.stdout, "offered package-bin\n");

    let show_pkg = run_cli(&["--registry", p, "show", "package-bin"], None, 0);
    assert_eq!(
        show_pkg.stdout,
        concat!(
            "id package-bin\n",
            "note ship artifact\n",
            "status open\n",
            "effective_status blocked\n",
            "priority 2\n",
            "tags linux,release\n",
            "depends_on build-core,deploy-app\n",
            "holder -\n",
            "expires_at -\n",
            "epoch 0\n",
            "takeover_count 0\n",
            "max_takeovers 3\n",
        )
    );

    // CLI offer-batch validation failures exit 2 on malformed /2 types/values and exit 1 on unknown deps & cycles.
    for bad_line in [
        "{\"id\": \"x\", \"priority\": true}\n",
        "{\"id\": \"x\", \"priority\": null}\n",
        "{\"id\": \"x\", \"tags\": null}\n",
        "{\"id\": \"x\", \"depends_on\": null}\n",
        "{\"id\": \"x\", \"depends_on\": [\"x\"]}\n",
        "{\"id\": \"x\", \"max_takeovers\": 0}\n",
    ] {
        run_cli(
            &["--registry", p, "offer-batch"],
            Some(bad_line.as_bytes()),
            2,
        );
    }
    run_cli(
        &["--registry", p, "offer-batch"],
        Some(b"{\"id\": \"x\", \"depends_on\": [\"unknown\"]}\n"),
        1,
    );
    let batch_cyc = run_cli(
        &["--registry", p, "offer-batch"],
        Some(
            b"{\"id\": \"cyc-1\", \"depends_on\": [\"cyc-2\"]}\n{\"id\": \"cyc-2\", \"depends_on\": [\"cyc-1\"]}\n",
        ),
        1,
    );
    assert_eq!(
        batch_cyc.stderr,
        "error: item dependencies contain a cycle\n"
    );

    // 3. CLI board (human) shows open and blocked effective statuses in offer order.
    let board_h = run_cli(&["--registry", p, "board"], None, 0);
    assert_eq!(
        board_h.stdout,
        concat!(
            "build-core open epoch=0\n",
            "deploy-app blocked epoch=0\n",
            "test-suite blocked epoch=0\n",
            "package-bin blocked epoch=0\n",
        )
    );

    // 4. CLI ready (human and --json) with --tag filtering.
    let ready_h = run_cli(&["--registry", p, "ready", "--tag", "rust"], None, 0);
    assert_eq!(ready_h.stdout, "build-core open epoch=0\n");
    let ready_none = run_cli(&["--registry", p, "ready", "--tag", "prod"], None, 0);
    assert_eq!(ready_none.stdout, "");

    let ready_j = run_cli(&["--registry", p, "ready", "--json"], None, 0);
    let Value::Array(ready_arr) = serde_json::from_str::<Value>(ready_j.stdout.trim()).unwrap()
    else {
        panic!("expected array");
    };
    assert_eq!(ready_arr.len(), 1);
    assert_eq!(
        object_field(&ready_arr[0], "id"),
        &Value::Str("build-core".to_string())
    );
    assert_eq!(object_field(&ready_arr[0], "priority"), &Value::Int(0));

    // 5. CLI claim on blocked item exits 1 with exact message.
    let blocked = run_cli(
        &[
            "--registry",
            p,
            "claim",
            "test-suite",
            "--holder",
            "alice",
            "--ttl",
            "60",
        ],
        None,
        1,
    );
    assert_eq!(
        blocked.stderr,
        "error: item 'test-suite' is blocked by incomplete dependencies\n"
    );

    // 6. Drive build-core to exhausted via library at controlled instants, then test CLI show/board/reset/history/compact!
    let reg = Registry::new(path.clone());
    let now_utc = Instant::now_utc().unwrap();
    let _g1 = reg
        .claim("build-core", "alice", 1, &now_utc.plus_seconds(-100))
        .unwrap();
    let g2 = reg
        .claim("build-core", "bob", 1, &now_utc.plus_seconds(-50))
        .unwrap();
    let hash2 = dibs::token_hash(&g2.token);

    let board_exh = run_cli(&["--registry", p, "board"], None, 0);
    assert_eq!(
        board_exh.stdout,
        concat!(
            "build-core exhausted epoch=2\n",
            "deploy-app blocked epoch=0\n",
            "test-suite blocked epoch=0\n",
            "package-bin blocked epoch=0\n",
        )
    );
    assert!(!board_exh.stdout.contains(&hash2));

    let show_exh = run_cli(&["--registry", p, "show", "build-core"], None, 0);
    assert!(show_exh.stdout.contains("status open\n"));
    assert!(show_exh.stdout.contains("effective_status exhausted\n"));
    assert!(show_exh.stdout.contains("takeover_count 1\n"));
    assert!(show_exh.stdout.contains("max_takeovers 1\n"));
    assert!(!show_exh.stdout.contains(&hash2));

    // CLI reset (--json)
    let reset_j = run_cli(&["--registry", p, "reset", "build-core", "--json"], None, 0);
    assert_eq!(
        reset_j.stdout,
        "{\"item\": \"build-core\", \"status\": \"open\", \"epoch\": 2, \"takeover_count\": 0}\n"
    );

    // CLI history (human) shows `<seq> reset item=build-core holder=- at=<ts> epoch=2 previous_holder=bob`
    let hist_h = run_cli(&["--registry", p, "history", "build-core"], None, 0);
    let last_hist = hist_h.stdout.lines().last().unwrap();
    assert!(
        last_hist.contains(" reset item=build-core holder=- at=")
            && last_hist.ends_with(" epoch=2 previous_holder=bob"),
        "{}",
        hist_h.stdout
    );
    assert!(!hist_h.stdout.contains(&hash2));

    // Also verify human `reset` output (`reset <id>\n`) after re-exhausting build-core:
    let _g_re = reg
        .claim("build-core", "alice", 1, &now_utc.plus_seconds(-40))
        .unwrap();
    let _g_re2 = reg
        .claim("build-core", "bob", 1, &now_utc.plus_seconds(-20))
        .unwrap();
    let reset_h = run_cli(&["--registry", p, "reset", "build-core"], None, 0);
    assert_eq!(reset_h.stdout, "reset build-core\n");

    // Spaced `--token` accepts 43-char base64url bearer tokens starting with `-` or `--`
    // (reaching protocol refusal at exit 1) while still rejecting real flags at exit 2.
    for hyphen_tok in [
        "-JveolMiNW_27DsyGcoWTAV_sCbZGXmToJoYAO9H8cE",
        "--veolMiNW_27DsyGcoWTAV_sCbZGXmToJoYAO9H8cE",
    ] {
        let hyp = run_cli(
            &[
                "--registry",
                p,
                "renew",
                "build-core",
                "--holder",
                "carol",
                "--token",
                hyphen_tok,
                "--epoch",
                "5",
                "--ttl",
                "60",
            ],
            None,
            1,
        );
        assert!(
            !hyp.stderr.contains("expected one argument"),
            "{}",
            hyp.stderr
        );
    }
    let missing_tok = run_cli(
        &[
            "--registry",
            p,
            "renew",
            "build-core",
            "--holder",
            "carol",
            "--token",
            "--epoch",
            "5",
            "--ttl",
            "60",
        ],
        None,
        2,
    );
    assert!(missing_tok
        .stderr
        .contains("argument --token: expected one argument"));

    // CLI claim-next --tag rust claims build-core now that it has been reset!
    let cn = run_cli(
        &[
            "--registry",
            p,
            "claim-next",
            "--holder",
            "carol",
            "--ttl",
            "600",
            "--tag",
            "rust",
            "--json",
        ],
        None,
        0,
    );
    let cn_val: Value = serde_json::from_str(cn.stdout.trim()).unwrap();
    assert_eq!(
        object_field(&cn_val, "item"),
        &Value::Str("build-core".to_string())
    );
    assert_eq!(object_field(&cn_val, "epoch"), &Value::Int(5));

    // CLI compact (--json and human)
    let comp_j = run_cli(
        &[
            "--registry",
            p,
            "compact",
            "--through-seq",
            "7",
            "--archive",
            a,
            "--json",
        ],
        None,
        0,
    );
    let comp_val: Value = serde_json::from_str(comp_j.stdout.trim()).unwrap();
    assert_eq!(
        object_keys(&comp_val),
        vec![
            "through_seq",
            "archived_count",
            "remaining_count",
            "archived_events_sha256",
            "archive_path"
        ]
    );
    assert_eq!(object_field(&comp_val, "through_seq"), &Value::Int(7));
    assert_eq!(object_field(&comp_val, "archived_count"), &Value::Int(7));
    assert_eq!(object_field(&comp_val, "remaining_count"), &Value::Int(4));
    assert!(!comp_j.stdout.contains(&hash2));

    let comp_h = run_cli(
        &[
            "--registry",
            p,
            "compact",
            "--through-seq=11",
            &format!("--archive={a}"),
        ],
        None,
        0,
    );
    assert_eq!(comp_h.stdout, "compacted 4 events through seq 11\n");

    // Non-UTF-8 `--archive` path on Unix works without panicking or rejecting valid OS bytes!
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let fresh_dir = temp_dir("cli-v2-raw-archive");
        let fresh_reg = fresh_dir.join("registry.json");
        let fresh_p = fresh_reg.to_str().unwrap();
        run_cli(&["--registry", fresh_p, "offer", "extra-item"], None, 0);
        let raw_arch = fresh_dir.join(OsStr::from_bytes(b"arch-\xff.jsonl"));
        if std::fs::write(&raw_arch, b"").is_ok() {
            std::fs::remove_file(&raw_arch).unwrap();
            let json_out = Command::new(env!("CARGO_BIN_EXE_dibs"))
                .arg("--json")
                .arg("--registry")
                .arg(&fresh_reg)
                .arg("compact")
                .arg("--through-seq")
                .arg("1")
                .arg("--archive")
                .arg(&raw_arch)
                .output()
                .unwrap();
            assert_eq!(json_out.status.code(), Some(2));
            assert_eq!(
                String::from_utf8_lossy(&json_out.stderr),
                "error: --json requires a UTF-8 --archive path\n"
            );
            assert!(!raw_arch.exists());

            let status = Command::new(env!("CARGO_BIN_EXE_dibs"))
                .arg("--registry")
                .arg(&fresh_reg)
                .arg("compact")
                .arg("--through-seq")
                .arg("1")
                .arg("--archive")
                .arg(&raw_arch)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(0));
            assert!(raw_arch.is_file());
        }
    }
}

#[test]
fn seq_ceiling_and_mid_life_compaction_edges() {
    let dir = temp_dir("seq-ceiling-and-mid-life");

    // 1. Sequence overflow fence at `u64::MAX` (`18446744073709551615`):
    // Stage a valid compacted `/2` registry at `through_seq = u64::MAX - 1` with empty history:
    // - 2-item `offer_many` refuses at exit 1 leaving the file byte-identical.
    // - 1-item `offer_with` succeeds at `seq = u64::MAX` and reads back cleanly!
    // - Once at `last_seq == u64::MAX`, EVERY mutating operation refuses at exit 1 leaving disk byte-identical!
    let ceil_path = dir.join("ceiling.json");
    let tok = "a".repeat(43);
    let tok_hash = dibs::token_hash(&tok);
    let ceil_doc = serde_json::json!({
        "format": "dibs-registry/2",
        "compaction": {
            "through_seq": u64::MAX - 1,
            "archived_events_sha256": "0".repeat(64),
            "compacted_at": "2031-05-04T12:00:00+00:00",
            "baseline": [
                {
                    "id": "exh-item",
                    "status": "open",
                    "holder": null,
                    "epoch": 2,
                    "takeover_count": 1
                },
                {
                    "id": "live-item",
                    "status": "claimed",
                    "holder": "alice",
                    "epoch": 1,
                    "takeover_count": 0
                },
                {
                    "id": "open-item",
                    "status": "open",
                    "holder": null,
                    "epoch": 0,
                    "takeover_count": 0
                }
            ]
        },
        "items": [
            {
                "id": "exh-item",
                "note": "",
                "status": "open",
                "priority": 2,
                "tags": [],
                "depends_on": [],
                "max_takeovers": 1,
                "takeover_count": 1,
                "holder": null,
                "token_hash": null,
                "epoch": 2,
                "expires_at": null
            },
            {
                "id": "live-item",
                "note": "",
                "status": "claimed",
                "priority": 2,
                "tags": [],
                "depends_on": [],
                "max_takeovers": null,
                "takeover_count": 0,
                "holder": "alice",
                "token_hash": tok_hash,
                "epoch": 1,
                "expires_at": "2031-05-04T13:00:00+00:00"
            },
            {
                "id": "open-item",
                "note": "",
                "status": "open",
                "priority": 2,
                "tags": [],
                "depends_on": [],
                "max_takeovers": null,
                "takeover_count": 0,
                "holder": null,
                "token_hash": null,
                "epoch": 0,
                "expires_at": null
            }
        ],
        "history": []
    });
    let raw_before = serde_json::to_string(&ceil_doc).unwrap();
    std::fs::write(&ceil_path, &raw_before).unwrap();
    let ceil_reg = Registry::new(ceil_path.clone());

    // At through_seq = u64::MAX - 1, a 2-item batch would assign u64::MAX and u64::MAX + 1 -> refused!
    let batch_over = expect_error(ceil_reg.offer_many(
        &[OfferSpec::new("new-1", ""), OfferSpec::new("new-2", "")],
        &t0(),
    ));
    assert!(matches!(batch_over, Error::Refusal(_)));
    assert_eq!(
        batch_over.message(),
        "history sequence has reached the 64-bit limit"
    );
    assert_eq!(std::fs::read_to_string(&ceil_path).unwrap(), raw_before);

    // A 1-item mutation succeeds and assigns exact `seq = u64::MAX`!
    ceil_reg.offer("at-max-seq", "", &t0()).unwrap();
    let hist_at_max = ceil_reg.history(None).unwrap();
    assert_eq!(hist_at_max.len(), 1);
    assert_eq!(hist_at_max[0].seq, u64::MAX as i128);
    assert_eq!(ceil_reg.board(&t0()).unwrap().len(), 4);

    // Now last_seq == u64::MAX: EVERY mutating operation refuses at exit 1 and leaves bytes unchanged!
    let raw_at_max = std::fs::read_to_string(&ceil_path).unwrap();
    for err in [
        expect_error(ceil_reg.offer("overflow-offer", "", &at(1))),
        expect_error(ceil_reg.offer_many(&[OfferSpec::new("overflow-batch", "")], &at(1))),
        expect_error(ceil_reg.claim("open-item", "bob", 60, &at(1))),
        expect_error(ceil_reg.claim_next("bob", 60, &at(1))),
        expect_error(ceil_reg.renew("live-item", "alice", &tok, 1, 60, &at(1))),
        expect_error(ceil_reg.release("live-item", "alice", &tok, 1, &at(1))),
        expect_error(ceil_reg.complete("live-item", "alice", &tok, 1, &at(1))),
        expect_error(ceil_reg.reset("exh-item", &at(1))),
    ] {
        assert!(matches!(err, Error::Refusal(_)), "{err:?}");
        assert_eq!(err.exit_code(), 1);
        assert_eq!(
            err.message(),
            "history sequence has reached the 64-bit limit"
        );
        assert_eq!(std::fs::read_to_string(&ceil_path).unwrap(), raw_at_max);
    }

    // 2. Uncompacted mid-life `/2` registries compact cleanly:
    // Case A: pre-history takeovers + `taken_over` + `reset` in archived prefix, AND
    // Case B: mid-life items whose first event in `remaining_events` is `reset` (both expired-claimed and open)!
    let mid_path = dir.join("mid-life.json");
    let mid_arch = dir.join("mid-life.jsonl");
    let mid_doc = serde_json::json!({
        "format": "dibs-registry/2",
        "compaction": null,
        "items": [
            {
                "id": "mid-archived",
                "note": "",
                "status": "open",
                "priority": 2,
                "tags": [],
                "depends_on": [],
                "max_takeovers": 2,
                "takeover_count": 0,
                "holder": null,
                "token_hash": null,
                "epoch": 3,
                "expires_at": null
            },
            {
                "id": "mid-rem-claimed",
                "note": "",
                "status": "open",
                "priority": 2,
                "tags": [],
                "depends_on": [],
                "max_takeovers": 1,
                "takeover_count": 0,
                "holder": null,
                "token_hash": null,
                "epoch": 2,
                "expires_at": null
            },
            {
                "id": "mid-rem-open",
                "note": "",
                "status": "open",
                "priority": 2,
                "tags": [],
                "depends_on": [],
                "max_takeovers": 1,
                "takeover_count": 0,
                "holder": null,
                "token_hash": null,
                "epoch": 2,
                "expires_at": null
            },
            {
                "id": "mid-no-reset",
                "note": "",
                "status": "claimed",
                "priority": 2,
                "tags": [],
                "depends_on": [],
                "max_takeovers": 3,
                "takeover_count": 2,
                "holder": "carol",
                "token_hash": tok_hash,
                "epoch": 3,
                "expires_at": "2031-05-04T13:00:00+00:00"
            }
        ],
        "history": [
            {"seq": 1, "kind": "taken_over", "item": "mid-archived", "holder": "bob", "at": "2031-05-04T12:00:00+00:00", "epoch": 3, "previous_holder": "alice"},
            {"seq": 2, "kind": "reset", "item": "mid-archived", "holder": null, "at": "2031-05-04T12:00:20+00:00", "epoch": 3, "previous_holder": "bob"},
            {"seq": 3, "kind": "taken_over", "item": "mid-no-reset", "holder": "carol", "at": "2031-05-04T12:00:25+00:00", "epoch": 3, "previous_holder": "bob"},
            {"seq": 4, "kind": "reset", "item": "mid-rem-claimed", "holder": null, "at": "2031-05-04T12:00:30+00:00", "epoch": 2, "previous_holder": "bob"},
            {"seq": 5, "kind": "reset", "item": "mid-rem-open", "holder": null, "at": "2031-05-04T12:00:35+00:00", "epoch": 2, "previous_holder": null}
        ]
    });
    std::fs::write(&mid_path, serde_json::to_string(&mid_doc).unwrap()).unwrap();
    let mid_reg = Registry::new(mid_path);
    assert_eq!(mid_reg.board(&t0()).unwrap().len(), 4);

    // Compact through seq 2: `mid-archived` is archived across its takeover+reset, `mid-no-reset` had
    // initial takeover_count 1 before seq 3, and `mid-rem-claimed` / `mid-rem-open` have `reset` in remaining_events!
    let c_mid = mid_reg.compact(2, &mid_arch, &at(60)).unwrap();
    assert_eq!(c_mid.through_seq, 2);
    assert_eq!(c_mid.remaining_count, 3);
    assert_eq!(mid_reg.board(&at(61)).unwrap().len(), 4);
    let c_mid_all = mid_reg.compact(5, &mid_arch, &at(62)).unwrap();
    assert_eq!(c_mid_all.through_seq, 5);
    assert_eq!(c_mid_all.remaining_count, 0);
    assert_eq!(mid_reg.board(&at(63)).unwrap().len(), 4);

    // 3. Legacy `/1` registry containing an eventless `status: "completed", epoch: 0` item
    // compacts cleanly and preserves that terminal baseline entry!
    let leg_path = dir.join("legacy-v1-done.json");
    let leg_arch = dir.join("legacy-v1-done.jsonl");
    let leg_doc = serde_json::json!({
        "format": "dibs-registry/1",
        "items": [
            {
                "id": "legacy-done",
                "note": "completed before history window",
                "status": "completed",
                "holder": null,
                "token_hash": null,
                "epoch": 0,
                "expires_at": null
            },
            {
                "id": "active-item",
                "note": "",
                "status": "open",
                "holder": null,
                "token_hash": null,
                "epoch": 0,
                "expires_at": null
            }
        ],
        "history": [
            {"seq": 1, "kind": "offered", "item": "active-item", "holder": null, "at": "2031-05-04T12:00:00+00:00", "epoch": 0, "previous_holder": null}
        ]
    });
    std::fs::write(&leg_path, serde_json::to_string(&leg_doc).unwrap()).unwrap();
    let leg_reg = Registry::new(leg_path);
    let c_leg = leg_reg.compact(1, &leg_arch, &at(70)).unwrap();
    assert_eq!(c_leg.through_seq, 1);
    assert_eq!(leg_reg.board(&at(71)).unwrap().len(), 1);
    let done_detail = leg_reg.show("legacy-done", &at(71)).unwrap();
    assert_eq!(done_detail.status, "completed");
    assert_eq!(done_detail.epoch, 0);
}
