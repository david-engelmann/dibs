"""dibs, expiring claim leases for shared agent backlogs with a Rust core.

This package is the thin Python face of the dibs library. Each
function calls the library. Claim, claim_next, renew, show, and
compact return one dictionary of the facts the command line reports,
board, ready, and history return lists of dictionaries, offer_many
returns the list of offered item ids, and offer, release, complete,
and reset return nothing. A refusal raises a subclass of DibsError
carrying the same message text the command line prints. InvalidNameError
covers an item, holder, or tag name in the wrong shape, InvalidTtlError
a time to live outside the bound, InvalidRequestError a request in
the wrong shape, NotFoundError an item missing from the backlog,
RefusalError a lease or item state that does not stand as the call
needs it, CorruptError a registry that cannot be read as recorded,
and FailureError a failure of the machine under the registry,
mirroring the error classes of the library itself.

Every operation that judges a lease accepts an optional current
instant as ISO 8601 text parsed by the library, and defaults to
the UTC wall clock when now is omitted or None. History judges no
lease and takes no instant. A claim token appears only in the
return of claim and claim_next, and neither a token nor its hash
appears in board, ready, show, history, compact, or an exception.
"""

import os
from typing import Iterable, List, Optional, TypedDict, Union

from ._dibs import (
    CorruptError,
    DibsError,
    FailureError,
    InvalidNameError,
    InvalidRequestError,
    InvalidTtlError,
    NotFoundError,
    RefusalError,
    _offer,
    board,
    claim,
    claim_next,
    compact,
    complete,
    history,
    offer_many,
    ready,
    release,
    renew,
    reset,
    show,
)

StrPath = Union[str, os.PathLike[str]]


class _OfferSpecRequired(TypedDict):
    id: str


class OfferSpecDict(_OfferSpecRequired, total=False):
    note: Optional[str]
    priority: int
    tags: Iterable[str]
    depends_on: Iterable[str]
    max_takeovers: Optional[int]


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
    holder: Optional[str]
    expires_at: Optional[str]
    epoch: int


class ItemDetailDict(TypedDict):
    id: str
    note: str
    status: str
    effective_status: str
    priority: int
    tags: List[str]
    depends_on: List[str]
    holder: Optional[str]
    expires_at: Optional[str]
    epoch: int
    takeover_count: int
    max_takeovers: Optional[int]


class EventDict(TypedDict):
    seq: int
    kind: str
    item: str
    holder: Optional[str]
    at: str
    epoch: int
    previous_holder: Optional[str]


class CompactSummaryDict(TypedDict):
    through_seq: int
    archived_count: int
    remaining_count: int
    archived_events_sha256: str
    archive_path: str


def offer(
    registry,
    item,
    now=None,
    note=None,
    *,
    priority=2,
    tags=None,
    depends_on=None,
    max_takeovers=None,
):
    """Add an item to the backlog and record the offered event."""
    return _offer(
        registry,
        item,
        now,
        note,
        priority=priority,
        tags=tags,
        depends_on=depends_on,
        max_takeovers=max_takeovers,
    )


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

__version__ = "1.0.0"
