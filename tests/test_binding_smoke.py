"""Smoke tests for the Python binding over the dibs library.

The suite drives the full claim lifecycle through the binding
against a real registry the library builds on disk, and checks
the facts each call returns against each other and against the
registry file itself. The cargo suite remains the gate for the
library itself, and this file is the consumer proof that a
Python caller can drive the same lifecycle the command line
drives, at instants the caller names outright.
"""

import hashlib

import pytest

import dibs

T0 = "2031-05-04T12:00:00+00:00"
T1 = "2031-05-04T12:30:00+00:00"


def rows_by_id(rows):
    return {row["id"]: row for row in rows}


class ForgedBytesInt(int):
    """An int whose to_bytes hands back the bytes of a small number."""

    def to_bytes(self, length=1, byteorder="big", *, signed=False):
        return int.to_bytes(60, 8, byteorder, signed=True)


class EmptyBytesInt(int):
    """An int whose to_bytes hands back no bytes at all."""

    def to_bytes(self, length=1, byteorder="big", *, signed=False):
        return b""


class ForgedBitLengthInt(int):
    """An int whose bit_length reports a small width."""

    def bit_length(self):
        return 8


def test_the_full_lifecycle_through_the_binding(tmp_path):
    registry = str(tmp_path / "registry.json")

    dibs.offer(registry, "task-one", T0, note="first task")
    dibs.offer(registry, "task-two", T0)

    granted = dibs.claim(registry, "task-one", "alice", 3600, T0)
    assert granted["item"] == "task-one"
    assert granted["holder"] == "alice"
    assert granted["epoch"] == 1
    assert granted["expires_at"] == "2031-05-04T13:00:00+00:00"
    token = granted["token"]
    assert isinstance(token, str)
    assert len(token) == 43

    board = rows_by_id(dibs.board(registry, T0))
    assert board["task-one"]["status"] == "claimed"
    assert board["task-one"]["holder"] == "alice"
    assert board["task-one"]["expires_at"] == "2031-05-04T13:00:00+00:00"
    assert board["task-one"]["epoch"] == 1
    assert board["task-one"]["note"] == "first task"
    assert board["task-two"]["status"] == "open"
    assert board["task-two"]["holder"] is None
    assert board["task-two"]["expires_at"] is None

    renewed = dibs.renew(registry, "task-one", "alice", token, 1, 3600, T1)
    assert renewed["expires_at"] == "2031-05-04T14:00:00+00:00"
    board = rows_by_id(dibs.board(registry, T1))
    assert board["task-one"]["expires_at"] == "2031-05-04T14:00:00+00:00"

    digest = hashlib.sha256(token.encode()).hexdigest()
    board = dibs.board(registry, T1)
    events = dibs.history(registry)
    assert token not in repr(board)
    assert digest not in repr(board)
    assert token not in repr(events)
    assert digest not in repr(events)
    assert [event["kind"] for event in events] == [
        "offered",
        "offered",
        "claimed",
        "renewed",
    ]
    assert [event["seq"] for event in events] == [1, 2, 3, 4]

    at_expiry = rows_by_id(dibs.board(registry, "2031-05-04T14:00:00+00:00"))
    assert at_expiry["task-one"]["status"] == "open"
    assert at_expiry["task-one"]["holder"] is None

    taken = dibs.claim(
        registry, "task-one", "bob", 60, "2031-05-04T14:00:00+00:00"
    )
    assert taken["epoch"] == 2
    assert taken["expires_at"] == "2031-05-04T14:01:00+00:00"
    events = dibs.history(registry, "task-one")
    assert events[-1]["kind"] == "taken_over"
    assert events[-1]["holder"] == "bob"
    assert events[-1]["previous_holder"] == "alice"
    assert events[-1]["epoch"] == 2

    dibs.complete(
        registry,
        "task-one",
        "bob",
        taken["token"],
        2,
        "2031-05-04T14:00:30+00:00",
    )
    assert "task-one" not in rows_by_id(
        dibs.board(registry, "2031-05-04T14:00:30+00:00")
    )
    with pytest.raises(dibs.RefusalError) as refused:
        dibs.claim(
            registry, "task-one", "carol", 60, "2031-05-04T14:00:30+00:00"
        )
    assert str(refused.value) == (
        "item 'task-one' is completed and accepts no claim"
    )
    events = dibs.history(registry, "task-one")
    assert events[-1]["kind"] == "completed"


def test_refusals_raise_with_the_library_messages(tmp_path):
    registry = str(tmp_path / "registry.json")
    dibs.offer(registry, "task-one", T0)
    granted = dibs.claim(registry, "task-one", "alice", 3600, T0)
    token = granted["token"]

    with pytest.raises(dibs.RefusalError) as wrong_token:
        dibs.renew(registry, "task-one", "alice", "not the token", 1, 60, T0)
    assert str(wrong_token.value) == (
        "the token presented for item 'task-one' does not match the lease"
    )
    assert token not in str(wrong_token.value)

    with pytest.raises(dibs.RefusalError) as wrong_epoch:
        dibs.release(registry, "task-one", "alice", token, 7, T0)
    assert str(wrong_epoch.value) == (
        "the epoch 7 presented for item 'task-one' does not match "
        "the standing epoch 1"
    )

    with pytest.raises(dibs.RefusalError) as oversized_epoch:
        dibs.release(registry, "task-one", "alice", token, 2**200, T0)
    assert str(oversized_epoch.value) == (
        "the epoch 170141183460469231731687303715884105727 presented for "
        "item 'task-one' does not match the standing epoch 1"
    )

    with pytest.raises(dibs.InvalidTtlError) as capped_ttl:
        dibs.claim(registry, "task-two", "alice", 10**5000, T0)
    assert str(capped_ttl.value).startswith("invalid time to live 1")
    assert str(capped_ttl.value).endswith(
        "use a positive integer number of seconds, at most 31536000"
    )

    with pytest.raises(dibs.InvalidNameError) as invalid:
        dibs.offer(registry, "Not A Name", T0)
    assert str(invalid.value) == (
        "invalid item id 'Not A Name', use one component of lowercase "
        "letters and digits and hyphens and underscores, starting and "
        "ending with a letter or a digit, at most 64 characters long"
    )

    with pytest.raises(dibs.InvalidTtlError) as bad_ttl:
        dibs.claim(registry, "task-two", "alice", 0, T0)
    assert str(bad_ttl.value) == (
        "invalid time to live 0, use a positive integer number of "
        "seconds, at most 31536000"
    )

    with pytest.raises(dibs.NotFoundError) as missing:
        dibs.claim(registry, "task-nine", "alice", 60, T0)
    assert str(missing.value) == "item 'task-nine' does not stand in the backlog"

    with pytest.raises(dibs.InvalidRequestError) as naive:
        dibs.board(registry, "2031-05-04T12:00:00")
    assert str(naive.value) == (
        "the current instant must be a timezone aware datetime"
    )

    with pytest.raises(dibs.InvalidRequestError) as unparseable:
        dibs.board(registry, "not an instant")
    assert str(unparseable.value) == (
        "the current instant holds unparseable text 'not an instant'"
    )

    with pytest.raises(dibs.DibsError):
        dibs.claim(registry, "task-nine", "alice", 60, T0)

    dibs.release(registry, "task-one", "alice", token, 1, T0)
    board = rows_by_id(dibs.board(registry, T0))
    assert board["task-one"]["status"] == "open"


def test_integer_arguments_refuse_booleans(tmp_path):
    registry = str(tmp_path / "registry.json")
    dibs.offer(registry, "task-one", T0)
    granted = dibs.claim(registry, "task-one", "alice", 3600, T0)

    with pytest.raises(TypeError) as ttl_bool:
        dibs.claim(registry, "task-two", "alice", True, T0)
    assert "ttl" in str(ttl_bool.value)

    with pytest.raises(TypeError) as epoch_bool:
        dibs.renew(
            registry, "task-one", "alice", granted["token"], False, 60, T0
        )
    assert "epoch" in str(epoch_bool.value)

    board = rows_by_id(dibs.board(registry, T0))
    assert "task-two" not in board
    assert board["task-one"]["status"] == "claimed"
    assert board["task-one"]["expires_at"] == "2031-05-04T13:00:00+00:00"


def test_wide_integer_digits_come_from_the_value_not_its_methods(tmp_path):
    registry = str(tmp_path / "registry.json")
    dibs.offer(registry, "task-one", T0)

    whole = "1" + "0" * 5000
    tail = ", use a positive integer number of seconds, at most 31536000"

    with pytest.raises(dibs.InvalidTtlError) as forged:
        dibs.claim(registry, "task-two", "alice", ForgedBytesInt(10**5000), T0)
    assert str(forged.value) == "invalid time to live " + whole + tail

    with pytest.raises(dibs.InvalidTtlError) as empty:
        dibs.claim(registry, "task-two", "alice", EmptyBytesInt(10**5000), T0)
    assert str(empty.value) == "invalid time to live " + whole + tail

    with pytest.raises(dibs.InvalidTtlError) as negative:
        dibs.claim(registry, "task-two", "alice", -(10**5000), T0)
    assert str(negative.value) == "invalid time to live -" + whole + tail

    with pytest.raises(dibs.InvalidTtlError) as forged_negative:
        dibs.claim(
            registry, "task-two", "alice", ForgedBytesInt(-(10**5000)), T0
        )
    assert str(forged_negative.value) == "invalid time to live -" + whole + tail

    with pytest.raises(dibs.InvalidTtlError) as forged_width:
        dibs.claim(
            registry, "task-two", "alice", ForgedBitLengthInt(10**5000), T0
        )
    assert str(forged_width.value) == "invalid time to live " + whole + tail

    with pytest.raises(dibs.InvalidTtlError) as negative_width:
        dibs.claim(
            registry, "task-two", "alice", ForgedBitLengthInt(-(10**5000)), T0
        )
    assert str(negative_width.value) == "invalid time to live -" + whole + tail

    board = rows_by_id(dibs.board(registry, T0))
    assert "task-two" not in board
    assert board["task-one"]["status"] == "open"


def test_a_foreign_format_document_raises_corrupt_error(tmp_path):
    # DB-S3 mechanical update (DB-21): "dibs-registry/2" is now a supported
    # format version, so pinning the foreign-format refusal uses
    # "dibs-registry/99" and asserts both supported format names.
    registry = tmp_path / "registry.json"
    registry.write_text(
        '{"format": "dibs-registry/99", "items": [], "history": []}',
        encoding="utf-8",
    )
    with pytest.raises(dibs.CorruptError) as corrupt:
        dibs.board(str(registry), T0)
    assert str(corrupt.value) == (
        "the registry is corrupt, the format name is 'dibs-registry/99', "
        "not 'dibs-registry/1' or 'dibs-registry/2'"
    )


def test_a_registry_path_that_is_a_directory_raises_failure_error(tmp_path):
    with pytest.raises(dibs.FailureError) as failure:
        dibs.board(str(tmp_path), T0)
    assert str(failure.value).startswith("the registry file cannot be read")


def test_default_wall_clock_now_none_lifecycle(tmp_path):
    registry = str(tmp_path / "registry.json")

    dibs.offer(registry, "task-zero", None, note="zero note")
    offered = dibs.offer_many(
        registry,
        ["task-one", {"id": "task-two", "note": "second note"}],
        now=None,
    )
    assert offered == ["task-one", "task-two"]

    first = dibs.claim(registry, "task-zero", "alice", 3600, now=None)
    assert set(first.keys()) == {"item", "holder", "token", "epoch", "expires_at"}
    assert first["item"] == "task-zero"
    assert first["epoch"] == 1

    second = dibs.claim_next(registry, "bob", 3600)
    assert set(second.keys()) == {"item", "holder", "token", "epoch", "expires_at"}
    assert second["item"] == "task-one"
    assert second["holder"] == "bob"
    assert second["epoch"] == 1

    renewed = dibs.renew(registry, "task-zero", "alice", first["token"], 1, 600)
    assert "expires_at" in renewed

    rows = dibs.board(registry, now=None)
    assert len(rows) == 3
    for row in rows:
        assert list(row.keys()) == [
            "id",
            "note",
            "status",
            "holder",
            "expires_at",
            "epoch",
        ]

    detail = dibs.show(registry, "task-zero", now=None)
    assert list(detail.keys()) == [
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
    ]
    assert detail["status"] == "claimed"
    assert detail["effective_status"] == "claimed"
    assert detail["priority"] == 2
    assert detail["tags"] == []
    assert detail["depends_on"] == []
    assert detail["holder"] == "alice"
    assert detail["epoch"] == 1
    assert detail["takeover_count"] == 0
    assert detail["max_takeovers"] is None

    dibs.release(registry, "task-one", "bob", second["token"], 1)
    dibs.complete(registry, "task-zero", "alice", first["token"], 1)
    done_detail = dibs.show(registry, "task-zero")
    assert done_detail["status"] == "completed"
    assert done_detail["effective_status"] == "completed"
    assert done_detail["holder"] is None
    assert done_detail["expires_at"] is None


def test_offer_many_atomicity_and_validation(tmp_path):
    registry_path = tmp_path / "registry.json"
    registry = str(registry_path)

    for bare in [
        "task-bare-string",
        b"task-bare-bytes",
        bytearray(b"task-one"),
        {"id": "task-one", "note": "x"},
    ]:
        with pytest.raises(TypeError):
            dibs.offer_many(registry, bare, T0)
        assert not registry_path.exists()

    with pytest.raises(dibs.InvalidRequestError):
        dibs.offer_many(registry, [], T0)
    assert not registry_path.exists()

    with pytest.raises(dibs.InvalidNameError):
        dibs.offer_many(registry, ["Bad Name"], T0)
    assert not registry_path.exists()

    with pytest.raises(dibs.InvalidRequestError):
        dibs.offer_many(registry, ["task-one"], "2031-05-04T12:00:00")
    assert not registry_path.exists()

    with pytest.raises(dibs.InvalidRequestError):
        dibs.offer_many(registry, ["dup-item", "dup-item"], T0)
    assert not registry_path.exists()

    with pytest.raises(dibs.InvalidRequestError):
        dibs.offer_many(registry, [{"id": "task-one", "extra": 1}], T0)
    assert not registry_path.exists()

    dibs.offer(registry, "standing-item", T0)
    with pytest.raises(dibs.RefusalError) as dup_refusal:
        dibs.offer_many(
            registry,
            ["fresh-a", "fresh-b", "standing-item", "fresh-c"],
            T0,
        )
    assert str(dup_refusal.value) == "item 'standing-item' already stands in the backlog"
    assert [r["id"] for r in dibs.board(registry, T0)] == ["standing-item"]


def test_claim_next_and_show_lazy_expiry_and_takeover(tmp_path):
    registry = str(tmp_path / "registry.json")

    with pytest.raises(TypeError) as bool_ttl:
        dibs.claim_next(registry, "alice", True, T0)
    assert "ttl" in str(bool_ttl.value)

    with pytest.raises(dibs.RefusalError) as empty_refusal:
        dibs.claim_next(registry, "alice", 60, T0)
    assert str(empty_refusal.value) == "no ready item stands in the backlog"

    with pytest.raises(dibs.NotFoundError):
        dibs.show(registry, "missing-item", T0)

    dibs.offer_many(registry, [{"id": "task-a", "note": "n"}], T0)
    g1 = dibs.claim_next(registry, "alice", 60, T0)
    digest1 = hashlib.sha256(g1["token"].encode()).hexdigest()

    with pytest.raises(dibs.RefusalError):
        dibs.claim_next(registry, "bob", 60, "2031-05-04T12:00:59+00:00")

    at_expiry_detail = dibs.show(registry, "task-a", "2031-05-04T12:01:00+00:00")
    assert at_expiry_detail["status"] == "open"
    assert at_expiry_detail["effective_status"] == "open"
    assert at_expiry_detail["holder"] is None
    assert at_expiry_detail["expires_at"] is None
    assert at_expiry_detail["takeover_count"] == 0
    assert g1["token"] not in repr(at_expiry_detail)
    assert digest1 not in repr(at_expiry_detail)

    g2 = dibs.claim_next(registry, "bob", 60, "2031-05-04T12:01:00+00:00")
    assert g2["item"] == "task-a"
    assert g2["epoch"] == 2
    after_detail = dibs.show(registry, "task-a", "2031-05-04T12:01:01+00:00")
    assert after_detail["status"] == "claimed"
    assert after_detail["effective_status"] == "claimed"
    assert after_detail["holder"] == "bob"
    assert after_detail["epoch"] == 2
    assert after_detail["takeover_count"] == 1


def test_v2_dag_priorities_tags_and_ready_through_the_binding(tmp_path):
    registry_path = tmp_path / "registry.json"
    registry = str(registry_path)

    # Default offer keeps dibs-registry/1 byte-compatible on disk.
    dibs.offer(registry, "v1-plain", T0, note="plain")
    assert '"format": "dibs-registry/1"' in registry_path.read_text(encoding="utf-8")

    # Type and range rejections on v2 offer and offer_many arguments.
    with pytest.raises(TypeError):
        dibs.offer(registry, "bad-bool-prio", T0, priority=True)
    with pytest.raises(TypeError):
        dibs.offer(registry, "bad-none-prio", T0, priority=None)
    with pytest.raises(TypeError):
        dibs.offer(registry, "bad-bool-cap", T0, max_takeovers=False)
    with pytest.raises(TypeError):
        dibs.offer(registry, "bad-str-tags", T0, tags="linux")
    with pytest.raises(TypeError):
        dibs.offer(registry, "bad-bytes-deps", T0, depends_on=b"v1-plain")
    with pytest.raises(dibs.InvalidRequestError):
        dibs.offer(registry, "bad-prio-4", T0, priority=4)
    with pytest.raises(dibs.InvalidRequestError):
        dibs.offer(registry, "self-dep", T0, depends_on=["self-dep"])
    for bad_spec in [
        {"id": "x", "priority": None},
        {"id": "x", "tags": None},
        {"id": "x", "depends_on": None},
        {"id": "x", "max_takeovers": 0},
    ]:
        with pytest.raises(dibs.InvalidRequestError):
            dibs.offer_many(registry, [bad_spec], T0)
    with pytest.raises(TypeError):
        dibs.offer_many(registry, [{"id": "x", "priority": False}], T0)

    # {"id": ..., "note": None} is accepted consistently with dibs.offer(..., note=None)!
    assert dibs.offer_many(registry, [{"id": "note-none", "note": None}], T0) == [
        "note-none"
    ]
    assert dibs.show(registry, "note-none", T0)["note"] == ""
    # Re-initialize fresh registry so v1-plain + DAG test starts from clean state:
    registry_path.unlink()
    dibs.offer(registry, "v1-plain", T0, note="plain")

    # Validation order on claim_next: holder before ttl before tags!
    with pytest.raises(dibs.InvalidNameError) as ord_holder:
        dibs.claim_next(registry, "BAD HOLDER", 0, T0, tags=["bad tag"])
    assert str(ord_holder.value).startswith("invalid holder id 'BAD HOLDER'")
    with pytest.raises(dibs.InvalidTtlError):
        dibs.claim_next(registry, "alice", 0, T0, tags=["bad tag"])

    # Intra-batch forward DAG dependencies, priority ordering, and tag normalization,
    # including partial-tag priority-0 controls (`gpu-only` and `linux-only`) offered BEFORE `build-lib`!
    offered = dibs.offer_many(
        registry,
        [
            {
                "id": "gpu-only",
                "priority": 0,
                "tags": ["gpu"],
            },
            {
                "id": "linux-only",
                "priority": 0,
                "tags": ["linux"],
            },
            {
                "id": "deploy",
                "priority": 0,
                "tags": ["prod", "linux"],
                "depends_on": ["test-suite"],
            },
            {
                "id": "test-suite",
                "priority": 1,
                "tags": ["gpu", "linux", "gpu"],
                "depends_on": ["build-lib"],
                "max_takeovers": None,
            },
            {
                "id": "build-lib",
                "priority": 0,
                "tags": ["linux", "gpu"],
            },
        ],
        T0,
    )
    assert offered == ["gpu-only", "linux-only", "deploy", "test-suite", "build-lib"]
    assert '"format": "dibs-registry/2"' in registry_path.read_text(encoding="utf-8")

    with pytest.raises(dibs.RefusalError) as cyc:
        dibs.offer_many(
            registry,
            [
                {"id": "cyc-a", "depends_on": ["cyc-b"]},
                {"id": "cyc-b", "depends_on": ["cyc-a"]},
            ],
            T0,
        )
    assert str(cyc.value) == "item dependencies contain a cycle"

    # Blocked item refuses direct claim; board reports 6-key rows with effective status.
    with pytest.raises(dibs.RefusalError) as blocked_err:
        dibs.claim(registry, "deploy", "alice", 60, T0)
    assert str(blocked_err.value) == "item 'deploy' is blocked by incomplete dependencies"

    board_map = rows_by_id(dibs.board(registry, T0))
    assert board_map["deploy"]["status"] == "blocked"
    assert board_map["test-suite"]["status"] == "blocked"
    assert board_map["build-lib"]["status"] == "open"

    # ready orders by (priority ASC, offer_order ASC) and filters by ALL tags.
    all_ready = dibs.ready(registry, T0)
    assert [d["id"] for d in all_ready] == [
        "gpu-only",
        "linux-only",
        "build-lib",
        "v1-plain",
    ]
    assert list(all_ready[2].keys()) == [
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
    ]
    assert all_ready[2]["tags"] == ["gpu", "linux"]

    # Multi-tag AND filter excludes BOTH `gpu-only` and `linux-only` even though both stood earlier at priority 0!
    gpu_ready = dibs.ready(registry, T0, tags=["linux", "gpu"])
    assert [d["id"] for d in gpu_ready] == ["build-lib"]

    c_build = dibs.claim_next(registry, "worker-1", 60, T0, tags=["gpu", "linux"])
    assert c_build["item"] == "build-lib"
    dibs.complete(registry, "build-lib", "worker-1", c_build["token"], 1, T0)

    # Completing build-lib unblocks test-suite while deploy stays blocked until test-suite completes.
    assert [d["id"] for d in dibs.ready(registry, T0, tags=["linux"])] == [
        "linux-only",
        "test-suite",
    ]


def test_v2_max_takeovers_reset_and_idempotent_compact(tmp_path):
    registry = str(tmp_path / "registry.json")
    archive_path = tmp_path / "archive.jsonl"
    archive = str(archive_path)

    dibs.offer(registry, "flaky", T0, priority=1, max_takeovers=1)
    with pytest.raises(dibs.RefusalError):
        dibs.reset(registry, "flaky", T0)

    g1 = dibs.claim(registry, "flaky", "alice", 10, T0)
    assert g1["epoch"] == 1
    g2 = dibs.claim(registry, "flaky", "bob", 10, "2031-05-04T12:00:15+00:00")
    assert g2["epoch"] == 2

    # At bob's expiry ("2031-05-04T12:00:25+00:00"), flaky becomes exhausted.
    t_exh = "2031-05-04T12:00:25+00:00"
    assert dibs.show(registry, "flaky", t_exh)["effective_status"] == "exhausted"
    assert rows_by_id(dibs.board(registry, t_exh))["flaky"]["status"] == "exhausted"
    with pytest.raises(dibs.RefusalError) as exh_err:
        dibs.claim(registry, "flaky", "carol", 60, t_exh)
    assert str(exh_err.value) == "item 'flaky' has reached max_takeovers and must be reset"

    dibs.reset(registry, "flaky", "2031-05-04T12:00:26+00:00")
    after_reset = dibs.show(registry, "flaky", "2031-05-04T12:00:26+00:00")
    assert after_reset["effective_status"] == "open"
    assert after_reset["takeover_count"] == 0
    assert after_reset["epoch"] == 2

    g3 = dibs.claim(registry, "flaky", "carol", 60, "2031-05-04T12:00:27+00:00")
    assert g3["epoch"] == 3
    dibs.complete(
        registry, "flaky", "carol", g3["token"], 3, "2031-05-04T12:00:28+00:00"
    )

    # Validation checks on compact
    with pytest.raises(TypeError):
        dibs.compact(registry, True, archive, T1)
    with pytest.raises(dibs.InvalidRequestError):
        dibs.compact(registry, 0, archive, T1)
    with pytest.raises(dibs.InvalidRequestError):
        dibs.compact(registry, 2, registry, T1)

    # Initial compact through seq 3 (leaves seq 4, 5, 6 in registry)
    c1 = dibs.compact(registry, 3, archive, T1)
    assert list(c1.keys()) == [
        "through_seq",
        "archived_count",
        "remaining_count",
        "archived_events_sha256",
        "archive_path",
    ]
    assert c1["through_seq"] == 3
    assert c1["archived_count"] == 3
    assert c1["remaining_count"] == 3
    lines_1_3 = archive_path.read_bytes()
    assert hashlib.sha256(lines_1_3).hexdigest() == c1["archived_events_sha256"]

    # Simulate mid-compact crash where seq 4 was appended to archive before registry rename!
    ev4 = dibs.history(registry)[0]
    ev4_line = (
        f'{{"seq": {ev4["seq"]}, "kind": "{ev4["kind"]}", "item": "{ev4["item"]}", '
        f'"holder": null, "at": "{ev4["at"]}", "epoch": {ev4["epoch"]}, '
        f'"previous_holder": "{ev4["previous_holder"]}"}}\n'
    ).encode("utf-8")
    archive_path.write_bytes(lines_1_3 + ev4_line)

    c2 = dibs.compact(registry, 6, archive, T1)
    assert c2["through_seq"] == 6
    assert c2["archived_count"] == 3
    assert c2["remaining_count"] == 0
    full_bytes = archive_path.read_bytes()
    assert len(full_bytes.splitlines()) == 6
    assert hashlib.sha256(full_bytes).hexdigest() == c2["archived_events_sha256"]

    # Post-compaction offer continues at seq 7, and default wall-clock `now=None`
    # works across `ready`, `reset`, and `compact`!
    dibs.offer(registry, "post-compact", max_takeovers=1)
    assert [e["seq"] for e in dibs.history(registry)] == [7]
    ready_default_now = dibs.ready(registry)
    assert [d["id"] for d in ready_default_now] == ["post-compact"]

    g_wc1 = dibs.claim(registry, "post-compact", "alice", 1, "2020-01-01T00:00:00+00:00")
    assert g_wc1["epoch"] == 1
    g_wc2 = dibs.claim(registry, "post-compact", "bob", 60, now=None)
    assert g_wc2["epoch"] == 2
    dibs.release(registry, "post-compact", "bob", g_wc2["token"], 2)
    assert dibs.show(registry, "post-compact")["effective_status"] == "exhausted"
    dibs.reset(registry, "post-compact")
    assert dibs.show(registry, "post-compact")["effective_status"] == "open"
    c_wc = dibs.compact(registry, 11, archive)
    assert c_wc["through_seq"] == 11
    assert c_wc["remaining_count"] == 0
    assert isinstance(c_wc["archive_path"], str)
    assert c_wc["archive_path"] == archive

    # Lossless non-UTF-8 `archive_path` round-trip via `os.fsdecode` on Unix filesystems
    # that permit non-UTF-8 filenames:
    import os
    import sys

    if sys.platform != "win32":
        raw_reg = str(tmp_path / "raw-reg.json")
        dibs.offer(raw_reg, "raw-item", T0)
        raw_arch_bytes = os.fsencode(str(tmp_path)) + b"/arch-\xff.jsonl"
        try:
            with open(raw_arch_bytes, "wb"):
                pass
            os.unlink(raw_arch_bytes)
        except OSError:
            pass
        else:
            raw_arch_str = os.fsdecode(raw_arch_bytes)
            c_raw = dibs.compact(raw_reg, 1, raw_arch_str, T1)
            assert isinstance(c_raw["archive_path"], str)
            assert c_raw["archive_path"] == raw_arch_str
            assert os.fsencode(c_raw["archive_path"]) == raw_arch_bytes


