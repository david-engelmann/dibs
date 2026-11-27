"""dibs, expiring claim leases for shared agent backlogs with a Rust core.

This package is the thin Python face of the dibs library. Each
function calls the library. Claim, claim_next, renew, and show
return one dictionary of the facts the command line reports, board
and history return lists of dictionaries, offer_many returns the
list of offered item ids, and offer, release, and complete return
nothing. A refusal raises a subclass of DibsError carrying the
same message text the command line prints. InvalidNameError covers
an item or holder id in the wrong shape, InvalidTtlError a time to
live outside the bound, InvalidRequestError a request in the wrong
shape, NotFoundError an item missing from the backlog, RefusalError
a lease that does not stand as the call needs it, CorruptError a
registry that cannot be read as recorded, and FailureError a
failure of the machine under the registry, mirroring the error
classes of the library itself.

Every operation that judges a lease accepts an optional current
instant as ISO 8601 text parsed by the library, and defaults to
the UTC wall clock when now is omitted or None. History judges no
lease and takes no instant. A claim token appears only in the
return of claim and claim_next, and neither a token nor its hash
appears in board, show, history, or an exception.
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
    board,
    claim,
    claim_next,
    complete,
    history,
    offer,
    offer_many,
    release,
    renew,
    show,
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
    "complete",
    "history",
    "offer",
    "offer_many",
    "release",
    "renew",
    "show",
]

__version__ = "0.1.0"
