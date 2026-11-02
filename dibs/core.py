"""Core protocol logic for dibs, the expiring claim lease protocol.

A backlog lives in one registry file. The file holds one JSON
document under the format name dibs-registry/1, and the document
holds the items, their standing leases, and the full claim history,
so a state change and its history event publish in the same rename
and can never disagree. Every mutating operation holds an exclusive
flock on the lock file beside the registry across its whole read and
evaluate and modify and write cycle, and publishes by renaming a
complete temporary file over the registry. Reads take a shared lock
and open the registry fresh each time, and a reader never writes.

Expiry is lazy. There is no daemon and no timer anywhere in this
module. Every operation that evaluates lease state takes the
current instant as an argument and evaluates expiry against it,
and a lease is live at instants strictly before its expiry instant
and expired at the expiry instant itself. History returns the
recorded events and evaluates nothing.

Possession is proven by a token, not by a name. A successful claim
returns a fresh token drawn from the secrets module, the registry
stores only the SHA-256 hash of the token, and renew and release and
complete must present the token together with the standing epoch.
"""

import fcntl
import hashlib
import hmac
import json
import os
import re
import secrets
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

FORMAT_NAME = "dibs-registry/1"
LOCK_SUFFIX = ".lock"
NAME_MAX = 64
NOTE_MAX = 200
TOKEN_BYTES = 32
TTL_MAX = 365 * 24 * 60 * 60

NAME_PATTERN = re.compile(r"^[a-z0-9]([a-z0-9_-]*[a-z0-9])?$")
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")

DOCUMENT_KEYS = {"format", "items", "history"}
ITEM_KEYS = {"id", "note", "status", "holder", "token_hash", "epoch", "expires_at"}
EVENT_KEYS = {"seq", "kind", "item", "holder", "at", "epoch", "previous_holder"}
EVENT_KINDS = {"offered", "claimed", "renewed", "released", "completed", "taken_over"}
ITEM_STATUSES = {"open", "claimed", "completed"}


class DibsError(Exception):
    """An error the CLI reports with a nonzero exit and no traceback."""

    exit_code = 1


class InvalidNameError(DibsError):
    """An item id or a holder id is not in the shape the protocol accepts."""

    exit_code = 2


class InvalidTtlError(DibsError):
    """A time to live is not a positive integer number of seconds."""

    exit_code = 2


class InvalidRequestError(DibsError):
    """A request value, such as an instant or a note, is not usable."""

    exit_code = 2


class NotFoundError(DibsError):
    """The named item does not stand in the backlog."""


class RefusalError(DibsError):
    """The standing state of the backlog refuses the operation."""


class CorruptRegistryError(DibsError):
    """The registry document cannot be read as it was recorded."""


@dataclass
class ClaimGranted:
    """The one return of a claim, and the only place a token appears."""

    item: str
    holder: str
    token: str
    epoch: int
    expires_at: datetime


def _is_int(value):
    return type(value) is int


def _token_hash(token):
    return hashlib.sha256(token.encode("utf-8")).hexdigest()


def validate_name(value, kind):
    """Refuse any item id or holder id outside the protocol name rule."""
    if (
        not isinstance(value, str)
        or len(value) > NAME_MAX
        or not NAME_PATTERN.fullmatch(value)
    ):
        raise InvalidNameError(
            f"invalid {kind} id {value!r}, use one component of lowercase "
            f"letters and digits and hyphens and underscores, starting and "
            f"ending with a letter or a digit, at most {NAME_MAX} characters long"
        )
    return value


def _validate_ttl(ttl):
    if not _is_int(ttl) or ttl < 1 or ttl > TTL_MAX:
        raise InvalidTtlError(
            f"invalid time to live {ttl!r}, use a positive integer number "
            f"of seconds, at most {TTL_MAX}"
        )
    return ttl


def _expiry_after(moment, ttl):
    try:
        return moment + timedelta(seconds=ttl)
    except OverflowError:
        raise InvalidTtlError(
            f"a time to live of {ttl} seconds runs past the last "
            f"representable instant"
        ) from None


def _validate_now(now):
    if not isinstance(now, datetime) or now.tzinfo is None or now.utcoffset() is None:
        raise InvalidRequestError(
            "the current instant must be a timezone aware datetime"
        )
    return now


def _validate_note(note):
    if not isinstance(note, str) or len(note) > NOTE_MAX:
        raise InvalidRequestError(
            f"invalid note, use a string of at most {NOTE_MAX} characters"
        )
    return note


def format_instant(moment):
    return moment.astimezone(timezone.utc).isoformat()


_format_instant = format_instant


def _parse_instant(text, corrupt):
    if not isinstance(text, str):
        raise corrupt(f"an instant field holds {text!r} instead of text")
    try:
        moment = datetime.fromisoformat(text)
    except ValueError:
        raise corrupt(f"an instant field holds unparseable text {text!r}") from None
    if moment.tzinfo is None or moment.utcoffset() is None:
        raise corrupt(f"an instant field holds a naive instant {text!r}")
    return moment


class _DuplicateKeyError(ValueError):
    pass


def _object_without_duplicate_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise _DuplicateKeyError(key)
        result[key] = value
    return result


def _empty_document():
    return {"format": FORMAT_NAME, "items": [], "history": []}


def _corrupt(message):
    return CorruptRegistryError(f"the registry is corrupt, {message}")


def _validate_stored_name(value, field, subject):
    if (
        not isinstance(value, str)
        or len(value) > NAME_MAX
        or not NAME_PATTERN.fullmatch(value)
    ):
        raise _corrupt(f"{subject} holds an invalid {field} {value!r}")


def _validate_item(item):
    if not isinstance(item, dict) or set(item) != ITEM_KEYS:
        raise _corrupt("an item does not carry exactly the recorded fields")
    _validate_stored_name(item["id"], "item id", "an item")
    if not isinstance(item["note"], str) or len(item["note"]) > NOTE_MAX:
        raise _corrupt(f"item {item['id']!r} holds an invalid note")
    status = item["status"]
    if not isinstance(status, str) or status not in ITEM_STATUSES:
        raise _corrupt(f"item {item['id']!r} holds an unknown status {status!r}")
    holder = item["holder"]
    if holder is not None:
        _validate_stored_name(holder, "holder id", "an item")
    token_hash = item["token_hash"]
    if token_hash is not None and (
        not isinstance(token_hash, str) or not SHA256_PATTERN.fullmatch(token_hash)
    ):
        raise _corrupt(f"item {item['id']!r} holds an invalid token hash")
    if not _is_int(item["epoch"]) or item["epoch"] < 0:
        raise _corrupt(f"item {item['id']!r} holds an invalid epoch")
    if item["expires_at"] is not None:
        _parse_instant(item["expires_at"], _corrupt)
    lease_fields = (holder, token_hash, item["expires_at"])
    if status == "claimed":
        if any(field is None for field in lease_fields):
            raise _corrupt(f"item {item['id']!r} stands claimed without a full lease")
        if item["epoch"] < 1:
            raise _corrupt(f"item {item['id']!r} stands claimed with epoch 0")
    elif any(field is not None for field in lease_fields):
        raise _corrupt(f"item {item['id']!r} stands {status} while holding a lease")


def _validate_event(event, position):
    if not isinstance(event, dict) or set(event) != EVENT_KEYS:
        raise _corrupt("a history event does not carry exactly the recorded fields")
    if not _is_int(event["seq"]) or event["seq"] != position + 1:
        raise _corrupt("the history sequence does not grow by 1 from 1 without gaps")
    if not isinstance(event["kind"], str) or event["kind"] not in EVENT_KINDS:
        raise _corrupt(f"a history event holds an unknown kind {event['kind']!r}")
    _validate_stored_name(event["item"], "item id", "a history event")
    holder = event["holder"]
    if event["kind"] == "offered":
        if holder is not None:
            raise _corrupt("an offered event names a holder")
    elif holder is None:
        raise _corrupt(f"a {event['kind']} event names no holder")
    else:
        _validate_stored_name(holder, "holder id", "a history event")
    previous = event["previous_holder"]
    if event["kind"] == "taken_over":
        if previous is None:
            raise _corrupt("a taken over event names no previous holder")
        _validate_stored_name(previous, "holder id", "a history event")
    elif previous is not None:
        raise _corrupt("an event that is not a takeover names a previous holder")
    _parse_instant(event["at"], _corrupt)
    if not _is_int(event["epoch"]) or event["epoch"] < 0:
        raise _corrupt("a history event holds an invalid epoch")
    if event["kind"] == "offered" and event["epoch"] != 0:
        raise _corrupt("an offered event carries an epoch other than 0")
    if event["kind"] != "offered" and event["epoch"] < 1:
        raise _corrupt(f"a {event['kind']} event carries epoch 0")


def _validate_document(document):
    if not isinstance(document, dict) or set(document) != DOCUMENT_KEYS:
        raise _corrupt("the document does not carry exactly the recorded fields")
    if document["format"] != FORMAT_NAME:
        raise _corrupt(f"the format name is {document['format']!r}, not {FORMAT_NAME!r}")
    if not isinstance(document["items"], list):
        raise _corrupt("the items field is not a list")
    if not isinstance(document["history"], list):
        raise _corrupt("the history field is not a list")
    seen = set()
    for item in document["items"]:
        _validate_item(item)
        if item["id"] in seen:
            raise _corrupt(f"item {item['id']!r} stands in the backlog twice")
        seen.add(item["id"])
    for position, event in enumerate(document["history"]):
        _validate_event(event, position)
    return document


def _find_item(document, item_id):
    for item in document["items"]:
        if item["id"] == item_id:
            return item
    return None


class Registry:
    """One backlog in one registry file, opened fresh on every call."""

    def __init__(self, path):
        self.path = Path(path)
        self.lock_path = self.path.with_name(self.path.name + LOCK_SUFFIX)

    @contextmanager
    def _locked(self, exclusive):
        fd = os.open(self.lock_path, os.O_RDWR | os.O_CREAT, 0o644)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH)
            yield
        finally:
            fcntl.flock(fd, fcntl.LOCK_UN)
            os.close(fd)

    def _read_document(self):
        """Read and strictly validate the registry, or start empty.

        A missing file reads as the empty document. A file that stands
        but cannot be read as recorded raises CorruptRegistryError and
        is never reset and never written over by a read.
        """
        try:
            raw = self.path.read_bytes()
        except FileNotFoundError:
            return _empty_document()
        except OSError as exc:
            raise _corrupt(f"the registry file cannot be read, {exc}") from exc
        try:
            document = json.loads(raw, object_pairs_hook=_object_without_duplicate_keys)
        except _DuplicateKeyError as exc:
            raise _corrupt(f"a key stands twice in the document, {exc}") from exc
        except (json.JSONDecodeError, UnicodeDecodeError) as exc:
            raise _corrupt(f"the document does not parse, {exc}") from exc
        return _validate_document(document)

    def _publish(self, document):
        text = json.dumps(document, indent=2) + "\n"
        temp = self.path.with_name(
            f".{self.path.name}.tmp-{os.getpid()}-{secrets.token_hex(4)}"
        )
        try:
            with open(temp, "w", encoding="utf-8") as handle:
                handle.write(text)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temp, self.path)
        except BaseException:
            try:
                temp.unlink()
            except OSError:
                pass
            raise
        try:
            dir_fd = os.open(self.path.parent, os.O_RDONLY)
            try:
                os.fsync(dir_fd)
            finally:
                os.close(dir_fd)
        except OSError:
            pass

    def _mutate(self, now, action):
        _validate_now(now)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self._locked(exclusive=True):
            document = self._read_document()
            result = action(document)
            self._publish(document)
            return result

    def _read(self, action):
        if not self.path.exists():
            return action(_empty_document())
        with self._locked(exclusive=False):
            return action(self._read_document())

    def _append_event(self, document, kind, item, holder, now, epoch, previous=None):
        document["history"].append(
            {
                "seq": len(document["history"]) + 1,
                "kind": kind,
                "item": item,
                "holder": holder,
                "at": format_instant(now),
                "epoch": epoch,
                "previous_holder": previous,
            }
        )

    @staticmethod
    def _lease_live(item, now):
        return item["status"] == "claimed" and now < _parse_instant(
            item["expires_at"], _corrupt
        )

    def _standing_lease(self, document, item_id, now):
        """Return the item while its lease stands live at the given instant."""
        item = _find_item(document, item_id)
        if item is None:
            raise NotFoundError(f"item {item_id!r} does not stand in the backlog")
        if item["status"] == "completed":
            raise RefusalError(f"item {item_id!r} is completed and accepts no lease")
        if item["status"] != "claimed":
            raise RefusalError(f"item {item_id!r} stands open and holds no lease")
        if not self._lease_live(item, now):
            raise RefusalError(
                f"the lease on item {item_id!r} has expired and no longer stands"
            )
        return item

    @staticmethod
    def _check_possession(item, holder, token, epoch):
        if holder != item["holder"]:
            raise RefusalError(
                f"item {item['id']!r} is held by {item['holder']!r}, not by {holder!r}"
            )
        if not _is_int(epoch) or epoch != item["epoch"]:
            raise RefusalError(
                f"the epoch {epoch!r} presented for item {item['id']!r} does "
                f"not match the standing epoch {item['epoch']}"
            )
        if not isinstance(token, str) or not hmac.compare_digest(
            _token_hash(token), item["token_hash"]
        ):
            raise RefusalError(
                f"the token presented for item {item['id']!r} does not match the lease"
            )

    def offer(self, item_id, note, now):
        """Add an item to the backlog and record the offered event."""
        validate_name(item_id, "item")
        _validate_note(note)

        def action(document):
            if _find_item(document, item_id) is not None:
                raise RefusalError(
                    f"item {item_id!r} already stands in the backlog"
                )
            document["items"].append(
                {
                    "id": item_id,
                    "note": note,
                    "status": "open",
                    "holder": None,
                    "token_hash": None,
                    "epoch": 0,
                    "expires_at": None,
                }
            )
            self._append_event(document, "offered", item_id, None, now, 0)
            return None

        return self._mutate(now, action)

    def claim(self, item_id, holder, ttl, now):
        """Lease an open or expired item and return the one ClaimGranted."""
        validate_name(item_id, "item")
        validate_name(holder, "holder")
        _validate_ttl(ttl)

        def action(document):
            item = _find_item(document, item_id)
            if item is None:
                raise NotFoundError(
                    f"item {item_id!r} does not stand in the backlog"
                )
            if item["status"] == "completed":
                raise RefusalError(
                    f"item {item_id!r} is completed and accepts no claim"
                )
            kind = "claimed"
            previous = None
            if item["status"] == "claimed":
                if self._lease_live(item, now):
                    raise RefusalError(
                        f"item {item_id!r} is already claimed by {item['holder']!r}"
                    )
                kind = "taken_over"
                previous = item["holder"]
            token = secrets.token_urlsafe(TOKEN_BYTES)
            expires = _expiry_after(now, ttl)
            item["status"] = "claimed"
            item["holder"] = holder
            item["token_hash"] = _token_hash(token)
            item["epoch"] += 1
            item["expires_at"] = format_instant(expires)
            self._append_event(
                document, kind, item_id, holder, now, item["epoch"], previous
            )
            return ClaimGranted(item_id, holder, token, item["epoch"], expires)

        return self._mutate(now, action)

    def renew(self, item_id, holder, token, epoch, ttl, now):
        """Extend the standing lease from its current expiry and return it."""
        validate_name(item_id, "item")
        validate_name(holder, "holder")
        _validate_ttl(ttl)

        def action(document):
            item = self._standing_lease(document, item_id, now)
            self._check_possession(item, holder, token, epoch)
            expires = _expiry_after(_parse_instant(item["expires_at"], _corrupt), ttl)
            item["expires_at"] = format_instant(expires)
            self._append_event(document, "renewed", item_id, holder, now, item["epoch"])
            return expires

        return self._mutate(now, action)

    @staticmethod
    def _end_lease(item, status):
        item["status"] = status
        item["holder"] = None
        item["token_hash"] = None
        item["expires_at"] = None

    def release(self, item_id, holder, token, epoch, now):
        """End the standing lease early and return the item to open."""
        validate_name(item_id, "item")
        validate_name(holder, "holder")

        def action(document):
            item = self._standing_lease(document, item_id, now)
            self._check_possession(item, holder, token, epoch)
            self._end_lease(item, "open")
            self._append_event(document, "released", item_id, holder, now, item["epoch"])
            return None

        return self._mutate(now, action)

    def complete(self, item_id, holder, token, epoch, now):
        """Mark the item done under its standing lease. Terminal in slice 1."""
        validate_name(item_id, "item")
        validate_name(holder, "holder")

        def action(document):
            item = self._standing_lease(document, item_id, now)
            self._check_possession(item, holder, token, epoch)
            self._end_lease(item, "completed")
            self._append_event(
                document, "completed", item_id, holder, now, item["epoch"]
            )
            return None

        return self._mutate(now, action)

    def _board_row(self, item, now):
        if self._lease_live(item, now):
            return {
                "id": item["id"],
                "note": item["note"],
                "status": "claimed",
                "holder": item["holder"],
                "expires_at": item["expires_at"],
                "epoch": item["epoch"],
            }
        return {
            "id": item["id"],
            "note": item["note"],
            "status": "open",
            "holder": None,
            "expires_at": None,
            "epoch": item["epoch"],
        }

    def board(self, now):
        """Return the open items in offer order, evaluated at the instant."""
        _validate_now(now)

        def action(document):
            return [
                self._board_row(item, now)
                for item in document["items"]
                if item["status"] != "completed"
            ]

        return self._read(action)

    def history(self, item_id=None):
        """Return the recorded events in order, or those of one item."""
        if item_id is not None:
            validate_name(item_id, "item")

        def action(document):
            if item_id is not None and _find_item(document, item_id) is None:
                raise NotFoundError(
                    f"item {item_id!r} does not stand in the backlog"
                )
            return [
                dict(event)
                for event in document["history"]
                if item_id is None or event["item"] == item_id
            ]

        return self._read(action)
