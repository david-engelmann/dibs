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
