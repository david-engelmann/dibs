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
    "CorruptError",
    "DibsError",
    "FailureError",
    "InvalidNameError",
    "InvalidRequestError",
    "InvalidTtlError",
    "NotFoundError",
    "RefusalError",
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

__version__ = "0.1.0"
