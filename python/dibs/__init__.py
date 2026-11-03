"""dibs, expiring claim leases for shared agent backlogs with a Rust core.

This package is the thin Python face of the dibs library. Each
function calls the library and returns plain dictionaries of the
facts the command line reports, and a refusal raises a subclass
of DibsError carrying the same message text the command line
prints. InvalidNameError covers an item or holder id in the
wrong shape, InvalidTtlError a time to live outside the bound,
InvalidRequestError a request in the wrong shape, NotFoundError
an item missing from the backlog, RefusalError a lease that
does not stand as the call needs it, CorruptError a registry
that cannot be read as recorded, and FailureError a failure of
the machine under the registry, mirroring the error classes of
the library itself.

Every operation that judges a lease takes the current instant
as ISO 8601 text the library parses, exactly as the library
takes it, and no call reads a clock of its own. History judges
no lease and takes no instant. A claim token appears only in
the return of the claim that issues it, and neither a token nor
its hash appears in the board or the history or an exception.
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
    complete,
    history,
    offer,
    release,
    renew,
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
    "complete",
    "history",
    "offer",
    "release",
    "renew",
]

__version__ = "0.1.0"
