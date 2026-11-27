import os
import sys
from collections.abc import Iterable
from typing import TypedDict

if sys.version_info >= (3, 10):
    from typing import TypeAlias
else:
    from typing_extensions import TypeAlias

StrPath: TypeAlias = str | os.PathLike[str]

class DibsError(Exception): ...
class InvalidNameError(DibsError): ...
class InvalidTtlError(DibsError): ...
class InvalidRequestError(DibsError): ...
class NotFoundError(DibsError): ...
class RefusalError(DibsError): ...
class CorruptError(DibsError): ...
class FailureError(DibsError): ...

class _OfferSpecRequired(TypedDict):
    id: str

class OfferSpecDict(_OfferSpecRequired, total=False):
    note: str | None
    priority: int
    tags: Iterable[str]
    depends_on: Iterable[str]
    max_takeovers: int | None

class ClaimGrantedDict(TypedDict):
    item: str
    holder: str
    token: str
    epoch: int
    expires_at: str

class RenewedDict(TypedDict):
    expires_at: str

class BoardRowDict(TypedDict):
    id: str
    note: str
    status: str
    holder: str | None
    expires_at: str | None
    epoch: int

class ItemDetailDict(TypedDict):
    id: str
    note: str
    status: str
    effective_status: str
    priority: int
    tags: list[str]
    depends_on: list[str]
    holder: str | None
    expires_at: str | None
    epoch: int
    takeover_count: int
    max_takeovers: int | None

class EventDict(TypedDict):
    seq: int
    kind: str
    item: str
    holder: str | None
    at: str
    epoch: int
    previous_holder: str | None

class CompactSummaryDict(TypedDict):
    through_seq: int
    archived_count: int
    remaining_count: int
    archived_events_sha256: str
    archive_path: str

def offer(
    registry: StrPath,
    item: str,
    now: str | None = None,
    note: str | None = None,
    *,
    priority: int = 2,
    tags: Iterable[str] | None = None,
    depends_on: Iterable[str] | None = None,
    max_takeovers: int | None = None,
) -> None: ...
def offer_many(
    registry: StrPath,
    specs: Iterable[str | OfferSpecDict],
    now: str | None = None,
) -> list[str]: ...
def claim(
    registry: StrPath,
    item: str,
    holder: str,
    ttl: int,
    now: str | None = None,
) -> ClaimGrantedDict: ...
def claim_next(
    registry: StrPath,
    holder: str,
    ttl: int,
    now: str | None = None,
    *,
    tags: Iterable[str] | None = None,
) -> ClaimGrantedDict: ...
def renew(
    registry: StrPath,
    item: str,
    holder: str,
    token: str,
    epoch: int,
    ttl: int,
    now: str | None = None,
) -> RenewedDict: ...
def release(
    registry: StrPath,
    item: str,
    holder: str,
    token: str,
    epoch: int,
    now: str | None = None,
) -> None: ...
def complete(
    registry: StrPath,
    item: str,
    holder: str,
    token: str,
    epoch: int,
    now: str | None = None,
) -> None: ...
def reset(
    registry: StrPath,
    item: str,
    now: str | None = None,
) -> None: ...
def board(
    registry: StrPath,
    now: str | None = None,
) -> list[BoardRowDict]: ...
def ready(
    registry: StrPath,
    now: str | None = None,
    *,
    tags: Iterable[str] | None = None,
) -> list[ItemDetailDict]: ...
def show(
    registry: StrPath,
    item: str,
    now: str | None = None,
) -> ItemDetailDict: ...
def history(
    registry: StrPath,
    item: str | None = None,
) -> list[EventDict]: ...
def compact(
    registry: StrPath,
    through_seq: int,
    archive: StrPath,
    now: str | None = None,
) -> CompactSummaryDict: ...

__all__ = [
    "BoardRowDict",
    "ClaimGrantedDict",
    "CompactSummaryDict",
    "CorruptError",
    "DibsError",
    "EventDict",
    "FailureError",
    "InvalidNameError",
    "InvalidRequestError",
    "InvalidTtlError",
    "ItemDetailDict",
    "NotFoundError",
    "OfferSpecDict",
    "RefusalError",
    "RenewedDict",
    "StrPath",
    "__version__",
    "board",
    "claim",
    "claim_next",
    "compact",
    "complete",
    "history",
    "offer",
    "offer_many",
    "ready",
    "release",
    "renew",
    "reset",
    "show",
]
__version__: str
