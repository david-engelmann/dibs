"""Tests for the dibs claim lease protocol, slice 1.

Deterministic time comes only from injected instants. No test sleeps
to make a lease expire, and the only wall clock use is the command
line smoke path where no expiry boundary is crossed. Fixtures are
synthetic and written for this repo.
"""

import json
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from dibs import core  # noqa: E402
from dibs.core import (  # noqa: E402
    CorruptRegistryError,
    InvalidNameError,
    InvalidRequestError,
    InvalidTtlError,
    NotFoundError,
    RefusalError,
    Registry,
)

T0 = datetime(2031, 5, 4, 12, 0, 0, tzinfo=timezone.utc)


def at(seconds):
    return T0 + timedelta(seconds=seconds)


def make_registry(tmp_path):
    return Registry(tmp_path / "registry.json")


def run_cli(*argv, expect=0):
    result = subprocess.run(
        [sys.executable, "-m", "dibs", *argv],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    assert result.returncode == expect, result.stderr
    return result


def test_offer_claim_release_round_trip(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    granted = registry.claim("task-one", "agent-a", 60, at(1))
    assert granted.epoch == 1
    assert granted.expires_at == at(61)
    assert len(granted.token) == 43
    rows = registry.board(at(2))
    assert rows == [
        {
            "id": "task-one",
            "note": "a synthetic task",
            "status": "claimed",
            "holder": "agent-a",
            "expires_at": core._format_instant(at(61)),
            "epoch": 1,
        }
    ]
    registry.release("task-one", "agent-a", granted.token, 1, at(3))
    rows = registry.board(at(4))
    assert rows[0]["status"] == "open"
    assert rows[0]["holder"] is None
    assert rows[0]["expires_at"] is None
    assert rows[0]["epoch"] == 1


def test_full_life_history_is_gapless_and_typed(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    granted = registry.claim("task-one", "agent-a", 60, at(1))
    registry.renew("task-one", "agent-a", granted.token, 1, 60, at(30))
    registry.release("task-one", "agent-a", granted.token, 1, at(40))
    second = registry.claim("task-one", "agent-b", 60, at(50))
    registry.complete("task-one", "agent-b", second.token, 2, at(55))
    events = registry.history()
    assert [event["seq"] for event in events] == [1, 2, 3, 4, 5, 6]
    assert [event["kind"] for event in events] == [
        "offered",
        "claimed",
        "renewed",
        "released",
        "claimed",
        "completed",
    ]
    assert events[0]["holder"] is None
    assert events[0]["epoch"] == 0
    assert [event["epoch"] for event in events[1:]] == [1, 1, 1, 2, 2]
    assert registry.history("task-one") == events
    assert registry.board(at(56)) == []


def test_claim_on_a_live_lease_refuses_and_names_the_holder(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    registry.claim("task-one", "agent-a", 60, at(1))
    with pytest.raises(RefusalError) as caught:
        registry.claim("task-one", "agent-b", 60, at(2))
    assert "agent-a" in str(caught.value)


def test_expiry_boundary_is_the_expiry_instant_itself(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    registry.claim("task-one", "agent-a", 60, at(0))
    live = registry.board(at(59))
    assert live[0]["status"] == "claimed"
    boundary = registry.board(at(60))
    assert boundary[0]["status"] == "open"
    assert boundary[0]["holder"] is None
    assert boundary[0]["expires_at"] is None
    assert boundary[0]["epoch"] == 1


def test_renew_at_the_expiry_instant_is_refused(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    granted = registry.claim("task-one", "agent-a", 60, at(0))
    renewed = registry.renew("task-one", "agent-a", granted.token, 1, 30, at(59))
    assert renewed == at(90)
    registry_two = Registry(tmp_path / "second.json")
    registry_two.offer("task-two", "a synthetic task", T0)
    granted_two = registry_two.claim("task-two", "agent-a", 60, at(0))
    with pytest.raises(RefusalError):
        registry_two.renew("task-two", "agent-a", granted_two.token, 1, 30, at(60))


def test_renew_extends_from_the_current_expiry_not_from_now(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    granted = registry.claim("task-one", "agent-a", 100, at(0))
    expires = registry.renew("task-one", "agent-a", granted.token, 1, 25, at(90))
    assert expires == at(125)


def test_takeover_records_the_previous_holder_and_fences_the_old_lease(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    first = registry.claim("task-one", "agent-a", 60, at(0))
    second = registry.claim("task-one", "agent-b", 60, at(60))
    assert second.epoch == 2
    assert second.token != first.token
    events = registry.history("task-one")
    assert events[-1]["kind"] == "taken_over"
    assert events[-1]["holder"] == "agent-b"
    assert events[-1]["previous_holder"] == "agent-a"
    for attempt in (
        lambda: registry.renew("task-one", "agent-a", first.token, 1, 30, at(61)),
        lambda: registry.release("task-one", "agent-a", first.token, 1, at(61)),
        lambda: registry.complete("task-one", "agent-a", first.token, 1, at(61)),
    ):
        with pytest.raises(RefusalError):
            attempt()
    with pytest.raises(RefusalError):
        registry.renew("task-one", "agent-a", second.token, 1, 30, at(61))
    with pytest.raises(RefusalError):
        registry.release("task-one", "agent-b", second.token, 1, at(61))
    registry.release("task-one", "agent-b", second.token, 2, at(62))


def test_wrong_token_fails_even_with_the_right_holder_name(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    registry.claim("task-one", "agent-a", 60, at(0))
    with pytest.raises(RefusalError):
        registry.release("task-one", "agent-a", "not-the-token", 1, at(1))
    with pytest.raises(RefusalError):
        registry.complete("task-one", "agent-a", "not-the-token", 1, at(1))
    with pytest.raises(RefusalError):
        registry.renew("task-one", "agent-a", "not-the-token", 1, 30, at(1))


def test_wrong_holder_and_stale_epoch_fail(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    granted = registry.claim("task-one", "agent-a", 60, at(0))
    with pytest.raises(RefusalError):
        registry.release("task-one", "agent-b", granted.token, 1, at(1))
    with pytest.raises(RefusalError):
        registry.release("task-one", "agent-a", granted.token, 7, at(1))
    with pytest.raises(RefusalError):
        registry.release("task-one", "agent-a", granted.token, "1", at(1))


def test_epoch_mismatch_message_names_both_epochs_and_never_says_stale(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    granted = registry.claim("task-one", "agent-a", 60, at(0))
    with pytest.raises(RefusalError) as caught:
        registry.release("task-one", "agent-a", granted.token, 7, at(1))
    message = str(caught.value)
    assert "does not match the standing epoch 1" in message
    assert "7" in message
    assert "stale" not in message
    with pytest.raises(RefusalError) as caught:
        registry.renew("task-one", "agent-a", granted.token, "1", 30, at(1))
    message = str(caught.value)
    assert "does not match the standing epoch 1" in message
    assert "stale" not in message


def test_the_registry_stores_only_the_token_hash(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    granted = registry.claim("task-one", "agent-a", 60, at(0))
    raw = (tmp_path / "registry.json").read_bytes()
    assert granted.token.encode("utf-8") not in raw
    document = json.loads(raw)
    item = document["items"][0]
    assert item["token_hash"] == core._token_hash(granted.token)
    assert item["token_hash"] != granted.token


def test_board_and_history_output_carry_no_token_or_hash(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    granted = registry.claim("task-one", "agent-a", 60, at(0))
    board_text = json.dumps(registry.board(at(1)))
    history_text = json.dumps(registry.history())
    token_hash = core._token_hash(granted.token)
    assert granted.token not in board_text
    assert granted.token not in history_text
    assert token_hash not in board_text
    assert token_hash not in history_text


def test_completion_is_terminal_and_the_id_cannot_be_reused(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    granted = registry.claim("task-one", "agent-a", 60, at(0))
    registry.complete("task-one", "agent-a", granted.token, 1, at(1))
    for attempt in (
        lambda: registry.claim("task-one", "agent-b", 60, at(2)),
        lambda: registry.renew("task-one", "agent-a", granted.token, 1, 30, at(2)),
        lambda: registry.release("task-one", "agent-a", granted.token, 1, at(2)),
        lambda: registry.complete("task-one", "agent-a", granted.token, 1, at(2)),
        lambda: registry.offer("task-one", "a second life", at(2)),
    ):
        with pytest.raises(RefusalError):
            attempt()
    assert [event["kind"] for event in registry.history("task-one")] == [
        "offered",
        "claimed",
        "completed",
    ]
    assert registry.board(at(3)) == []


def test_open_item_refuses_lease_operations_and_unknown_item_is_not_found(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    for attempt in (
        lambda: registry.renew("task-one", "agent-a", "tok", 0, 30, at(1)),
        lambda: registry.release("task-one", "agent-a", "tok", 0, at(1)),
        lambda: registry.complete("task-one", "agent-a", "tok", 0, at(1)),
    ):
        with pytest.raises(RefusalError):
            attempt()
    for attempt in (
        lambda: registry.claim("task-ghost", "agent-a", 60, at(1)),
        lambda: registry.renew("task-ghost", "agent-a", "tok", 1, 30, at(1)),
        lambda: registry.release("task-ghost", "agent-a", "tok", 1, at(1)),
        lambda: registry.complete("task-ghost", "agent-a", "tok", 1, at(1)),
        lambda: registry.history("task-ghost"),
    ):
        with pytest.raises(NotFoundError):
            attempt()
    with pytest.raises(RefusalError):
        registry.offer("task-one", "a duplicate offer", at(1))


def test_a_second_claim_after_release_carries_a_fresh_token_and_epoch(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    first = registry.claim("task-one", "agent-a", 60, at(0))
    registry.release("task-one", "agent-a", first.token, 1, at(1))
    second = registry.claim("task-one", "agent-a", 60, at(2))
    assert second.epoch == 2
    assert second.token != first.token
    with pytest.raises(RefusalError):
        registry.release("task-one", "agent-a", first.token, 2, at(3))


BAD_NAMES = [
    "",
    "Upper",
    "has space",
    "-leading",
    "trailing-",
    "_leading",
    "trailing_",
    "a/b",
    "a.b",
    "a" * 65,
    7,
    None,
]


@pytest.mark.parametrize("bad", BAD_NAMES)
def test_invalid_item_names_are_refused_everywhere(tmp_path, bad):
    registry = make_registry(tmp_path)
    with pytest.raises(InvalidNameError):
        registry.offer(bad, "a synthetic task", T0)
    with pytest.raises(InvalidNameError):
        registry.claim(bad, "agent-a", 60, T0)
    if bad is not None:
        with pytest.raises(InvalidNameError):
            registry.history(bad)


@pytest.mark.parametrize("bad", BAD_NAMES)
def test_invalid_holder_names_are_refused(tmp_path, bad):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    with pytest.raises(InvalidNameError):
        registry.claim("task-one", bad, 60, T0)
    with pytest.raises(InvalidNameError):
        registry.renew("task-one", bad, "tok", 1, 30, T0)


@pytest.mark.parametrize("good", ["a", "0", "a-b_2", "agent-007", "a" * 64])
def test_valid_name_edges_are_accepted(tmp_path, good):
    registry = make_registry(tmp_path)
    registry.offer(good, "a synthetic task", T0)
    granted = registry.claim(good, good, 60, T0)
    assert granted.epoch == 1


@pytest.mark.parametrize("bad_ttl", [0, -5, 1.5, "60", True, None])
def test_invalid_times_to_live_are_refused(tmp_path, bad_ttl):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    with pytest.raises(InvalidTtlError):
        registry.claim("task-one", "agent-a", bad_ttl, T0)
    granted = registry.claim("task-one", "agent-a", 60, T0)
    with pytest.raises(InvalidTtlError):
        registry.renew("task-one", "agent-a", granted.token, 1, bad_ttl, at(1))


def test_ttl_ceiling_is_the_contract_number_and_is_enforced(tmp_path):
    assert core.TTL_MAX == 31536000
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    granted = registry.claim("task-one", "agent-a", core.TTL_MAX, T0)
    assert granted.expires_at == T0 + timedelta(seconds=core.TTL_MAX)
    with pytest.raises(InvalidTtlError):
        registry.claim("task-one", "agent-b", core.TTL_MAX + 1, at(1))
    with pytest.raises(InvalidTtlError):
        registry.renew(
            "task-one", "agent-a", granted.token, 1, core.TTL_MAX + 1, at(1)
        )


@pytest.mark.parametrize("huge", [10**12, 10**18])
def test_huge_ttl_is_refused_as_a_typed_error(tmp_path, huge):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "a synthetic task", T0)
    with pytest.raises(InvalidTtlError):
        registry.claim("task-one", "agent-a", huge, T0)
    granted = registry.claim("task-one", "agent-a", 60, T0)
    with pytest.raises(InvalidTtlError):
        registry.renew("task-one", "agent-a", granted.token, 1, huge, at(1))


def test_repeated_renewals_end_in_a_typed_error_at_the_representable_edge(tmp_path):
    start = datetime(9999, 1, 1, 0, 0, 0, tzinfo=timezone.utc)
    path = tmp_path / "registry.json"
    registry = Registry(path)
    registry.offer("task-one", "a synthetic task", start)
    granted = registry.claim("task-one", "agent-a", 8640000, start)
    renewals = 0
    with pytest.raises(InvalidTtlError):
        while True:
            registry.renew("task-one", "agent-a", granted.token, 1, 8640000, start)
            renewals += 1
            assert renewals < 10
    assert renewals == 2
    before = path.read_bytes()
    with pytest.raises(InvalidTtlError):
        registry.renew("task-one", "agent-a", granted.token, 1, 8640000, start)
    assert path.read_bytes() == before
    kinds = [event["kind"] for event in registry.history("task-one")]
    assert kinds == ["offered", "claimed", "renewed", "renewed"]


def test_note_bound_is_enforced_at_the_bound(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-one", "n" * 200, T0)
    with pytest.raises(InvalidRequestError):
        registry.offer("task-two", "n" * 201, T0)
    with pytest.raises(InvalidRequestError):
        registry.offer("task-three", 42, T0)


def test_a_naive_instant_is_refused(tmp_path):
    registry = make_registry(tmp_path)
    naive = datetime(2031, 5, 4, 12, 0, 0)
    with pytest.raises(InvalidRequestError):
        registry.offer("task-one", "a synthetic task", naive)
    with pytest.raises(InvalidRequestError):
        registry.board(naive)


def test_missing_registry_reads_empty_and_the_first_mutation_creates_it(tmp_path):
    path = tmp_path / "deep" / "nested" / "registry.json"
    registry = Registry(path)
    assert registry.board(T0) == []
    assert registry.history() == []
    assert not path.exists()
    assert not (tmp_path / "deep").exists()
    registry.offer("task-one", "a synthetic task", T0)
    assert path.is_file()
    assert [row["id"] for row in registry.board(T0)] == ["task-one"]


def test_board_lists_items_in_offer_order(tmp_path):
    registry = make_registry(tmp_path)
    for name in ("task-c", "task-a", "task-b"):
        registry.offer(name, "a synthetic task", T0)
    registry.claim("task-a", "agent-a", 60, at(1))
    rows = registry.board(at(2))
    assert [row["id"] for row in rows] == ["task-c", "task-a", "task-b"]
    assert [row["status"] for row in rows] == ["open", "claimed", "open"]


def fresh_valid_dir(base, name):
    directory = base / name
    directory.mkdir()
    registry = Registry(directory / "registry.json")
    registry.offer("task-one", "a synthetic task", T0)
    registry.claim("task-one", "agent-a", 60, at(0))
    return directory


def valid_text(directory):
    return (directory / "registry.json").read_text(encoding="utf-8")


def assert_corrupt_and_unchanged(directory, text):
    path = directory / "registry.json"
    path.write_text(text, encoding="utf-8")
    before = path.read_bytes()
    registry = Registry(path)
    for attempt in (
        lambda: registry.board(at(1)),
        lambda: registry.history(),
        lambda: registry.offer("task-two", "a synthetic task", at(1)),
        lambda: registry.claim("task-one", "agent-b", 60, at(1)),
    ):
        with pytest.raises(CorruptRegistryError):
            attempt()
    assert path.read_bytes() == before


def mutated(base, name, change):
    directory = fresh_valid_dir(base, name)
    document = json.loads(valid_text(directory))
    change(document)
    return directory, json.dumps(document)


def test_damage_duplicate_keys_at_any_level(tmp_path):
    root_dir = fresh_valid_dir(tmp_path, "root-case")
    text = valid_text(root_dir)
    raw_duplicate = text.replace(
        '"format": "dibs-registry/1"',
        '"format": "dibs-registry/1", "format": "dibs-registry/1"',
        1,
    )
    assert raw_duplicate != text
    assert_corrupt_and_unchanged(root_dir, raw_duplicate)
    nested_dir = fresh_valid_dir(tmp_path, "nested-case")
    nested = valid_text(nested_dir).replace(
        '"note": "a synthetic task"',
        '"note": "a synthetic task", "note": "a synthetic task"',
        1,
    )
    assert nested != valid_text(nested_dir)
    assert_corrupt_and_unchanged(nested_dir, nested)


def test_damage_wrong_format_name(tmp_path):
    directory, text = mutated(
        tmp_path, "case", lambda doc: doc.update(format="dibs-registry/2")
    )
    assert_corrupt_and_unchanged(directory, text)


def test_damage_wrong_types(tmp_path):
    cases = [
        lambda doc: doc.update(items={}),
        lambda doc: doc.update(history="none"),
        lambda doc: doc["items"][0].update(epoch="1"),
        lambda doc: doc["items"][0].update(epoch=True),
        lambda doc: doc["items"][0].update(holder=7),
        lambda doc: doc["items"][0].update(expires_at=99),
        lambda doc: doc["items"][0].update(expires_at="not an instant"),
        lambda doc: doc["items"][0].update(expires_at="2031-05-04T12:00:00"),
        lambda doc: doc["items"][0].update(status=[]),
        lambda doc: doc["history"][0].update(kind={}),
        lambda doc: doc["history"][0].update(seq="1"),
    ]
    for index, change in enumerate(cases):
        directory, text = mutated(tmp_path, f"case-{index}", change)
        assert_corrupt_and_unchanged(directory, text)


def test_damage_unknown_fields(tmp_path):
    cases = [
        lambda doc: doc.update(extra=1),
        lambda doc: doc["items"][0].update(priority=1),
        lambda doc: doc["history"][0].update(comment="hello"),
        lambda doc: doc["items"][0].pop("epoch"),
        lambda doc: doc.pop("history"),
    ]
    for index, change in enumerate(cases):
        directory, text = mutated(tmp_path, f"case-{index}", change)
        assert_corrupt_and_unchanged(directory, text)


def test_damage_inconsistent_lease_shapes(tmp_path):
    cases = [
        lambda doc: doc["items"][0].update(status="open"),
        lambda doc: doc["items"][0].update(status="paused"),
        lambda doc: doc["items"][0].update(token_hash=None),
        lambda doc: doc["items"][0].update(token_hash="not-a-hash"),
        lambda doc: doc["items"][0].update(epoch=0),
        lambda doc: doc["items"][0].update(id="Bad Name"),
    ]
    for index, change in enumerate(cases):
        directory, text = mutated(tmp_path, f"case-{index}", change)
        assert_corrupt_and_unchanged(directory, text)


def test_damage_history_shapes(tmp_path):
    cases = [
        lambda doc: doc["history"][1].update(seq=9),
        lambda doc: doc["history"][1].update(kind="stolen"),
        lambda doc: doc["history"][1].update(previous_holder="agent-z"),
        lambda doc: doc["history"][0].update(holder="agent-a"),
        lambda doc: doc["history"][0].update(epoch=3),
        lambda doc: doc["items"].append(dict(doc["items"][0])),
    ]
    for index, change in enumerate(cases):
        directory, text = mutated(tmp_path, f"case-{index}", change)
        assert_corrupt_and_unchanged(directory, text)


def test_damage_truncated_and_empty_documents(tmp_path):
    directory = fresh_valid_dir(tmp_path, "cut-case")
    text = valid_text(directory)
    assert_corrupt_and_unchanged(directory, text[: len(text) // 2])
    empty_dir = fresh_valid_dir(tmp_path, "empty-case")
    assert_corrupt_and_unchanged(empty_dir, "")
    list_dir = fresh_valid_dir(tmp_path, "list-case")
    assert_corrupt_and_unchanged(list_dir, "[1, 2, 3]")


def test_damage_event_holder_name_message_names_the_event(tmp_path):
    directory, text = mutated(
        tmp_path, "case", lambda doc: doc["history"][1].update(holder="Bad Name")
    )
    path = directory / "registry.json"
    path.write_text(text, encoding="utf-8")
    registry = Registry(path)
    with pytest.raises(CorruptRegistryError) as caught:
        registry.board(at(1))
    assert "a history event holds an invalid holder id" in str(caught.value)


def test_registry_survives_a_reopen_between_operations(tmp_path):
    first = make_registry(tmp_path)
    first.offer("task-one", "a synthetic task", T0)
    granted = first.claim("task-one", "agent-a", 60, at(0))
    second = make_registry(tmp_path)
    assert second.board(at(1))[0]["holder"] == "agent-a"
    second.release("task-one", "agent-a", granted.token, 1, at(2))
    assert first.board(at(3))[0]["status"] == "open"


WORKER = """
import sys
from datetime import datetime

sys.path.insert(0, sys.argv[1])
from dibs.core import RefusalError, Registry

mode = sys.argv[2]
index = int(sys.argv[3])
registry = Registry(sys.argv[4])
now = datetime.fromisoformat(sys.argv[5])
if mode == "same":
    item = "task-shared"
else:
    item = f"task-{index}"
try:
    granted = registry.claim(item, f"agent-{index}", 600, now)
except RefusalError:
    raise SystemExit(3)
with open(sys.argv[6], "w", encoding="utf-8") as handle:
    handle.write(granted.token)
"""


def race_claims(tmp_path, mode, count):
    worker = tmp_path / "worker.py"
    worker.write_text(WORKER, encoding="utf-8")
    registry_path = tmp_path / "registry.json"
    children = []
    for index in range(count):
        result_path = tmp_path / f"result-{index}.txt"
        children.append(
            subprocess.Popen(
                [
                    sys.executable,
                    str(worker),
                    str(ROOT),
                    mode,
                    str(index),
                    str(registry_path),
                    core._format_instant(at(10)),
                    str(result_path),
                ]
            )
        )
    codes = [child.wait() for child in children]
    return codes, registry_path


def test_race_on_one_item_produces_exactly_one_winner(tmp_path):
    registry = make_registry(tmp_path)
    registry.offer("task-shared", "a synthetic task", T0)
    codes, registry_path = race_claims(tmp_path, "same", 8)
    assert codes.count(0) == 1
    assert codes.count(3) == 7
    winner = codes.index(0)
    document = json.loads(registry_path.read_text(encoding="utf-8"))
    item = document["items"][0]
    assert item["holder"] == f"agent-{winner}"
    assert item["epoch"] == 1
    kinds = [event["kind"] for event in document["history"]]
    assert kinds == ["offered", "claimed"]
    token = (tmp_path / f"result-{winner}.txt").read_text(encoding="utf-8")
    registry.release("task-shared", f"agent-{winner}", token, 1, at(20))


def test_race_on_distinct_items_loses_no_updates(tmp_path):
    registry = make_registry(tmp_path)
    for index in range(8):
        registry.offer(f"task-{index}", "a synthetic task", T0)
    codes, registry_path = race_claims(tmp_path, "distinct", 8)
    assert codes == [0] * 8
    document = json.loads(registry_path.read_text(encoding="utf-8"))
    assert len(document["items"]) == 8
    assert all(item["status"] == "claimed" for item in document["items"])
    assert all(item["epoch"] == 1 for item in document["items"])
    seqs = [event["seq"] for event in document["history"]]
    assert seqs == list(range(1, 17))
    claimed = [event for event in document["history"] if event["kind"] == "claimed"]
    assert len(claimed) == 8
    rows = Registry(registry_path).board(at(20))
    assert len(rows) == 8
    assert all(row["status"] == "claimed" for row in rows)


def test_cli_round_trip_and_exit_codes(tmp_path):
    path = str(tmp_path / "registry.json")
    offered = run_cli("--registry", path, "offer", "task-one", "--note", "synthetic")
    assert offered.stdout == "offered task-one\n"
    empty = run_cli("--registry", path, "board")
    assert empty.stdout == "task-one open epoch=0\n"
    claimed = run_cli(
        "--registry", path, "claim", "task-one", "--holder", "agent-a", "--ttl", "600"
    )
    lines = claimed.stdout.splitlines()
    assert lines[0].startswith("token ")
    token = lines[0].split(" ", 1)[1]
    assert lines[1] == "epoch 1"
    board = run_cli("--registry", path, "board")
    assert "task-one claimed holder=agent-a" in board.stdout
    assert "epoch=1" in board.stdout
    assert token not in board.stdout
    renewed = run_cli(
        "--registry", path, "renew", "task-one", "--holder", "agent-a",
        "--token", token, "--epoch", "1", "--ttl", "60",
    )
    assert renewed.stdout.startswith("expires_at ")
    history = run_cli("--registry", path, "history")
    assert "1 offered item=task-one" in history.stdout
    assert "2 claimed item=task-one holder=agent-a" in history.stdout
    assert "3 renewed item=task-one holder=agent-a" in history.stdout
    assert token not in history.stdout
    refused = run_cli(
        "--registry", path, "claim", "task-one", "--holder", "agent-b",
        "--ttl", "60", expect=1,
    )
    assert refused.stderr.startswith("error: ")
    assert "agent-a" in refused.stderr
    assert refused.stdout == ""
    missing = run_cli(
        "--registry", path, "claim", "task-ghost", "--holder", "agent-b",
        "--ttl", "60", expect=1,
    )
    assert missing.stderr.startswith("error: ")
    bad_name = run_cli(
        "--registry", path, "offer", "Bad Name", expect=2
    )
    assert bad_name.stderr.startswith("error: ")
    bad_ttl = run_cli(
        "--registry", path, "claim", "task-one", "--holder", "agent-a",
        "--ttl", "0", expect=2,
    )
    assert bad_ttl.stderr.startswith("error: ")
    released = run_cli(
        "--registry", path, "release", "task-one", "--holder", "agent-a",
        "--token", token, "--epoch", "1",
    )
    assert released.stdout == "released task-one\n"


def test_cli_surface_carries_no_time_override_flag(tmp_path):
    help_text = run_cli("--help").stdout
    assert "--now" not in help_text
    assert "--at" not in help_text
    path = str(tmp_path / "registry.json")
    run_cli("--registry", path, "offer", "task-one", "--now", "2031-01-01", expect=2)


def test_cli_huge_ttl_prints_one_error_line_and_no_traceback(tmp_path):
    path = str(tmp_path / "registry.json")
    run_cli("--registry", path, "offer", "task-one")
    result = run_cli(
        "--registry", path, "claim", "task-one", "--holder", "agent-a",
        "--ttl", "1000000000000000000", expect=2,
    )
    assert result.stderr.startswith("error: ")
    assert "Traceback" not in result.stderr
    assert result.stderr.count("\n") == 1
    assert result.stdout == ""


def test_cli_completed_item_leaves_the_board_but_keeps_its_history(tmp_path):
    path = str(tmp_path / "registry.json")
    run_cli("--registry", path, "offer", "task-one")
    claimed = run_cli(
        "--registry", path, "claim", "task-one", "--holder", "agent-a", "--ttl", "600"
    )
    token = claimed.stdout.splitlines()[0].split(" ", 1)[1]
    done = run_cli(
        "--registry", path, "complete", "task-one", "--holder", "agent-a",
        "--token", token, "--epoch", "1",
    )
    assert done.stdout == "completed task-one\n"
    assert run_cli("--registry", path, "board").stdout == ""
    history = run_cli("--registry", path, "history", "task-one")
    assert "3 completed item=task-one holder=agent-a" in history.stdout
